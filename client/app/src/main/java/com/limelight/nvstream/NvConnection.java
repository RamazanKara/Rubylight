/**
 * Android streaming connection facade coordinating host preflight, launch/resume,
 * input dispatch, microphone capture, hardware telemetry, and session actions via
 * {@link NightjarSessionJni} and {@link MoonBridge}.
 */
package com.limelight.nvstream;

import android.app.ActivityManager;
import android.content.Context;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.audio.MicProcessing;
import com.limelight.binding.audio.MicrophoneCapture;
import com.limelight.binding.audio.MicrophoneStatus;
import com.limelight.binding.input.InputBatcher;
import com.limelight.binding.input.MouseButtonOwnership;
import com.limelight.binding.video.SharpColor;
import com.limelight.computers.HostNotices;
import com.limelight.computers.HostPackedModes;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.JniNativeApiTransport;
import com.limelight.nativeapi.KnownCapabilities;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiTransport;
import com.limelight.nativeapi.NativeAudio;
import com.limelight.nativeapi.NativeCapabilities;
import com.limelight.nativeapi.NativeSession;
import com.limelight.nvstream.av.audio.AudioRenderer;
import com.limelight.nvstream.av.video.VideoDecoderRenderer;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HardwareMetrics;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.HostHttpResponseException;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarSessionJni;
import com.limelight.preferences.PreferenceConfiguration;

import org.json.JSONObject;
import org.xmlpull.v1.XmlPullParserException;

import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;
import java.io.OutputStream;
import java.security.NoSuchAlgorithmException;
import java.security.SecureRandom;
import java.security.cert.X509Certificate;
import java.util.IdentityHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.Semaphore;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.Consumer;
import java.util.function.Supplier;

import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;

public class NvConnection {
    static final class SessionEndedException extends IOException {}

    final LimelightCryptoProvider cryptoProvider;
    final String uniqueId;
    final ConnectionContext context;

    NativeApiTransport nativeApiTransport = new JniNativeApiTransport();
    volatile boolean nativeHostControl, nativeCouchMenu, nativeUsbForwarding;
    volatile boolean nativeImageClipboard, nativeSessionGuests, nativeNightMode, nativeHeadphoneSurround;
    volatile boolean nativeQuickSuspend;

    private static Semaphore connectionAllowed = new Semaphore(1);
    static long connectionWaitMs = 15_000;

    final boolean isMonkey;
    final Context appContext;
    volatile ComputerDetails hostDetails = new ComputerDetails();
    NvHTTP http;
    NvHTTP foundationAbrHttp;
    private Thread connectionThread;
    volatile boolean stopRequested;
    boolean nativeStarted;
    volatile boolean nativeConnected;
    RoamingNetworkWatch roamingWatch;
    // The stream's network was lost or its route moved (RoamingNetworkWatch): jumbo passed on the old path.
    volatile boolean networkMoved;
    String packedHdrDropAtLaunch;
    long lastServerCommandTime;

    volatile ScheduledExecutorService inputPoller;
    private ScheduledFuture<?> inputPoll;
    private ScheduledExecutorService hardwareMetricsPoller;
    private NvHTTP hardwareMetricsHttp;
    private boolean hardwareMetricsUnavailable;
    private long hardwareMetricsPollerHandle;
    private long inputIntervalNs = InputBatcher.intervalNs(60);

    volatile boolean micEnabled, micPermitted, micMuted;
    MicrophoneCapture microphone;
    volatile MicProcessing micProcessing = MicProcessing.DEFAULT;
    final AtomicReference<List<MicProcessing.Effect>> micNotice = new AtomicReference<>();
    final AtomicBoolean micFailed = new AtomicBoolean();
    volatile Runnable micNoticeListener;

    static final class ControllerMicrophoneState {
        final boolean allowed, ready, hostMuted;

        ControllerMicrophoneState(boolean allowed, boolean ready, boolean hostMuted) {
            this.allowed = allowed;
            this.ready = allowed && ready;
            this.hostMuted = hostMuted;
        }
    }

    final Map<Object, ControllerMicrophoneState> controllerMicrophones = new IdentityHashMap<>();
    long microphoneAccessAt;
    boolean microphoneAccess;
    Supplier<String> controllerPreparation;

    final InputBatcher inputBatcher = new InputBatcher(motion -> {
        if (!canSendInput()) return;
        if (motion.kind == 0) MoonBridge.sendMouseMove((short) motion.x, (short) motion.y);
        else if (motion.kind == 1) MoonBridge.sendMousePosition((short) motion.x, (short) motion.y, motion.width, motion.height);
        else MoonBridge.sendMouseMoveAsMousePosition((short) motion.x, (short) motion.y, motion.width, motion.height);
    });

    final MouseButtonOwnership mouseButtonOwnership = new MouseButtonOwnership();

    public NvConnection(Context appContext, ComputerDetails.AddressTuple host, int httpsPort,
            String uniqueId, StreamConfiguration config, LimelightCryptoProvider cryptoProvider,
            X509Certificate serverCert) {
        this.appContext = appContext;
        this.cryptoProvider = cryptoProvider;
        this.uniqueId = uniqueId;
        this.context = new ConnectionContext();
        this.context.serverAddress = host;
        this.context.httpsPort = httpsPort;
        this.context.streamConfig = config;
        this.context.serverCert = serverCert;
        this.context.fecChoice = config.getFecChoice();
        this.context.rateChange = config.getRateChange();
        this.context.startBitrateKbps = config.getBitrate();
        this.context.riKey = generateRiAesKey();
        this.context.riKeyId = generateRiKeyId();
        this.isMonkey = ActivityManager.isUserAMonkey();
    }

    private static SecretKey generateRiAesKey() {
        try {
            KeyGenerator keyGen = KeyGenerator.getInstance("AES");
            keyGen.init(128);
            return keyGen.generateKey();
        } catch (NoSuchAlgorithmException e) {
            throw new RuntimeException(e);
        }
    }

    private static int generateRiKeyId() {
        return new SecureRandom().nextInt();
    }

    static boolean acquireConnection() throws InterruptedException {
        return connectionAllowed.tryAcquire(connectionWaitMs, TimeUnit.MILLISECONDS);
    }

    static void releaseConnection() {
        connectionAllowed.release();
    }

    public synchronized void start(final AudioRenderer audioRenderer,
            final VideoDecoderRenderer videoDecoderRenderer, final NvConnectionListener connectionListener) {
        if (connectionThread != null || stopRequested) return;
        connectionThread = new Thread(() -> NvConnectionPreflight.runConnection(
                this, audioRenderer, videoDecoderRenderer, connectionListener));
        connectionThread.start();
    }

    public void stop() {
        long metricsHandle;
        synchronized (this) {
            if (stopRequested) return;
            stopRequested = true;
            controllerMicrophones.clear();
            stopHardwareMetricsPolling();
            metricsHandle = hardwareMetricsPollerHandle;
            hardwareMetricsPollerHandle = 0;
            if (inputPoller != null) {
                inputPoller.shutdownNow();
                inputPoller = null;
            }
            inputBatcher.clear();
            if (connectionThread != null && !nativeStarted) connectionThread.interrupt();
            if (http != null) http.cancelPendingRequests();
            if (nativeStarted) MoonBridge.interruptConnection();
        }
        if (metricsHandle != 0) NightjarSessionJni.metricsPollerDestroy(metricsHandle);
        synchronized (MoonBridge.class) {
            NvConnectionAudio.stopMicrophone(this);
            if (roamingWatch != null) {
                roamingWatch.stop();
                roamingWatch = null;
            }
            if (nativeConnected) {
                nativeConnected = false;
                MoonBridge.stopConnection();
                MoonBridge.cleanupBridge();
                synchronized (this) { nativeStarted = false; }
                connectionAllowed.release();
            }
        }
    }

    public ComputerDetails getHostDetails() { return hostDetails; }
    public int getStartBitrateKbps() { return context.startBitrateKbps; }
    public String getLaunchWarningCode() { return context.launchWarningCode; }
    public String getLaunchWarningMessage() { return context.launchWarningMessage; }

    /**
     * The packet size a reconnect of this session keeps, as desktop's carried_packet_size: the negotiated
     * one, or the one this start was given when it negotiated none (negative: none). 0, which asks for
     * normal packets, once the network was lost or moved under the stream: another path may drop jumbo
     * packets whole.
     */
    public int resumePacketSize() {
        if (networkMoved) return 0;
        return context.negotiatedPacketSize > 0 ? context.negotiatedPacketSize : context.streamConfig.getResumePacketSize();
    }

    /** The network {@link #resumePacketSize} was negotiated on (Network.getNetworkHandle; 0: none). */
    public long resumePacketNetwork() {
        return context.negotiatedPacketSize > 0 ? context.packetNetwork : context.streamConfig.getResumePacketNetwork();
    }

    // --- Hardware Metrics Polling ---

    public synchronized boolean startHardwareMetricsPolling(Consumer<HardwareMetrics> listener) {
        if (stopRequested || hardwareMetricsUnavailable || !hostDetails.supportsHardwareMetrics()
                || context.streamConfig.getApp().getRole() == NvApp.Role.INPUT_ONLY) {
            return false;
        }
        if (hardwareMetricsPollerHandle == 0) hardwareMetricsPollerHandle = NightjarSessionJni.metricsPollerCreate();
        if (NightjarSessionJni.metricsPollerIsUnavailable(hardwareMetricsPollerHandle)) {
            hardwareMetricsUnavailable = true;
            return false;
        }
        if (hardwareMetricsPoller != null) return true;
        ScheduledExecutorService poller =
                Executors.newSingleThreadScheduledExecutor(r -> new Thread(r, "Host metrics"));
        hardwareMetricsPoller = poller;
        poller.execute(() -> pollHardwareMetrics(poller, listener));
        return true;
    }

    private void pollHardwareMetrics(ScheduledExecutorService poller, Consumer<HardwareMetrics> listener) {
        long handle;
        synchronized (this) {
            if (hardwareMetricsPoller != poller) return;
            if (hardwareMetricsPollerHandle == 0) hardwareMetricsPollerHandle = NightjarSessionJni.metricsPollerCreate();
            handle = hardwareMetricsPollerHandle;
        }
        long nowMs = System.nanoTime() / 1_000_000L;
        int generation = NightjarSessionJni.metricsPollerDue(handle, nowMs, true);
        if (generation < 0) {
            if (NightjarSessionJni.metricsPollerIsUnavailable(handle)) {
                synchronized (this) {
                    if (hardwareMetricsPoller == poller) {
                        hardwareMetricsUnavailable = true;
                        stopHardwareMetricsPolling();
                    }
                }
            } else {
                synchronized (this) {
                    if (hardwareMetricsPoller == poller) {
                        poller.schedule(() -> pollHardwareMetrics(poller, listener), 50, TimeUnit.MILLISECONDS);
                    }
                }
            }
            return;
        }

        HardwareMetrics metrics = null;
        int httpStatus = 0;
        boolean unsupported = false;
        try {
            NvHTTP request;
            synchronized (this) {
                if (hardwareMetricsPoller != poller) return;
                request = hardwareMetricsHttp;
            }
            if (request == null) {
                request = new NvHTTP(context.serverAddress, hostDetails.httpsPort, uniqueId,
                        context.serverCert, cryptoProvider);
                request.setHostCapabilities(hostDetails.caps);
                synchronized (this) {
                    if (hardwareMetricsPoller != poller) {
                        request.cancelPendingRequests();
                        return;
                    }
                    hardwareMetricsHttp = request;
                }
            }
            metrics = request.getHardwareMetrics();
        } catch (FileNotFoundException | HardwareMetrics.UnsupportedVersionException e) {
            unsupported = true;
        } catch (HostHttpResponseException e) {
            httpStatus = e.getErrorCode();
        } catch (IOException e) {
            // Optional telemetry ignores transient network errors.
        }

        long finishMs = System.nanoTime() / 1_000_000L;
        long[] step = NightjarSessionJni.metricsPollerFinishStep(
                handle, generation, finishMs, metrics != null,
                metrics != null ? metrics.sampledAtUnixMs : 0L,
                metrics != null ? metrics.intervalMs : 0, httpStatus, unsupported);
        boolean show = step != null && step.length >= 1 && step[0] != 0;
        long delayMs = step != null && step.length >= 2 ? step[1] : 10_000L;
        boolean permanentlyUnavailable = step != null && step.length >= 3 && step[2] != 0;

        synchronized (this) {
            if (hardwareMetricsPoller != poller) return;
            if (permanentlyUnavailable) {
                hardwareMetricsUnavailable = true;
                stopHardwareMetricsPolling();
            }
        }
        listener.accept(show ? metrics : null);
        synchronized (this) {
            if (hardwareMetricsPoller == poller) {
                poller.schedule(() -> pollHardwareMetrics(poller, listener), delayMs, TimeUnit.MILLISECONDS);
            }
        }
    }

    public synchronized void stopHardwareMetricsPolling() {
        if (hardwareMetricsPollerHandle != 0) NightjarSessionJni.metricsPollerStop(hardwareMetricsPollerHandle);
        if (hardwareMetricsHttp != null) {
            hardwareMetricsHttp.cancelPendingRequests();
            hardwareMetricsHttp = null;
        }
        if (hardwareMetricsPoller != null) {
            hardwareMetricsPoller.shutdownNow();
            hardwareMetricsPoller = null;
        }
    }

    // --- Launch, Negotiation & Preflight Facades ---

    static int negotiateVideoFormats(int formats, int serverFormats) {
        return NightjarSessionJni.negotiateVideoFormats(formats, serverFormats);
    }

    static boolean pyroWaveHdrLost(int commonFormats, int negotiatedFormats) {
        return NightjarSessionJni.pyroWaveHdrLost(commonFormats, negotiatedFormats);
    }

    static SharpColor.Request sharpColorRequest(int offered, boolean sdr10, boolean pyroWave,
            boolean packedEngine, int hostScm, int hostPacked, boolean packedRefusedBefore) {
        return NvConnectionLaunch.sharpColorRequest(
                offered, sdr10, pyroWave, packedEngine, hostScm, hostPacked, packedRefusedBefore);
    }

    static int formatsAfter(SharpColor.Pick pick, int offered, boolean hdr) {
        return NvConnectionLaunch.formatsAfter(pick, offered, hdr);
    }

    static void announcePackedHdrDrop(Context appContext, String uuid, NvConnectionListener listener, String reason) {
        LimeLog.warning("HDR not used with packed full chroma: " + reason);
        if (HostNotices.claim(appContext, uuid, HostNotices.Notice.PACKED_HDR_UNAVAILABLE)) {
            listener.displayTransientMessage(appContext.getString(R.string.sharp_color_hdr_dropped));
        }
    }

    static void noticePackedAfterStart(Context appContext, String uuid, NvConnectionListener listener,
            boolean negotiatedHdr, String dropAtLaunch, String dropAfter) {
        if (!negotiatedHdr) return;
        if (dropAfter == null) HostNotices.reset(appContext, uuid, HostNotices.Notice.PACKED_HDR_UNAVAILABLE);
        else if (!dropAfter.equals(dropAtLaunch)) announcePackedHdrDrop(appContext, uuid, listener, dropAfter);
    }

    static int hostPackedModesBeforeLaunch(Context appContext, ComputerDetails details) {
        int cached = HostPackedModes.cached(appContext, details.uuid);
        int modes = HostPackedModes.known(details.packed444Modes, cached);
        if (modes >= 0) {
            LimeLog.info("Packed full chroma: the PC offers modes 0x" + Integer.toHexString(modes)
                    + (details.packed444Modes >= 0 ? " (serverinfo)" : " (last DESCRIBE)"));
        }
        return modes;
    }

    static void rememberPackedModes(Context appContext, String uuid, int startResult, int describeModes) {
        if (NightjarSessionJni.isPacked444Refused(startResult)) {
            HostPackedModes.store(appContext, uuid, 0);
            HostPackedModes.markRefused(appContext, uuid);
        } else if (startResult == 0) {
            HostPackedModes.store(appContext, uuid, describeModes);
        }
    }

    static boolean probesFirstBitrate(ConnectionContext context) {
        return NvConnectionLaunch.probesFirstBitrate(context);
    }

    interface LinkProbe {
        double mbps() throws IOException;
    }

    static int firstBitrateKbps(ConnectionContext context, LinkProbe probe) {
        return NvConnectionLaunch.firstBitrateKbps(context, true, probe);
    }

    static int pyroWaveStartKbps(ConnectionContext context, int linkMbps) {
        return NightjarSessionJni.pyroWaveStartKbps(
                context.negotiatedVideoFormats, context.streamConfig.getBitrate(),
                context.streamConfig.isAutomaticBitrate(), context.negotiatedWidth,
                context.negotiatedHeight, context.streamConfig.getRefreshRate(), linkMbps,
                context.streamConfig.getBitrateLimitKbps());
    }

    static PyroWaveBitrateFloor.LinkNotice pyroWaveLinkNotice(ConnectionContext context, int linkMbps) {
        int[] raw = NightjarSessionJni.pyroWaveLinkNotice(
                context.negotiatedVideoFormats, context.negotiatedWidth, context.negotiatedHeight,
                context.streamConfig.getRefreshRate(), linkMbps,
                context.streamConfig.getBitrate(), context.streamConfig.isAutomaticBitrate());
        if (raw == null || raw.length < 2) return null;
        return new PyroWaveBitrateFloor.LinkNotice(raw[0], 0, raw[1]);
    }

    static boolean launchesBesideRunningGame(NvApp app) {
        return app.getControl() != NvApp.Control.NONE || app.isMicOnly();
    }

    static boolean launchApp(NvHTTP http, ConnectionContext context, String verb)
            throws IOException, XmlPullParserException {
        return NvConnectionLaunch.launchApp(http, context, verb);
    }

    boolean launchOrResume(NvHTTP h, ConnectionContext context, ComputerDetails details, NvApp app,
            String serverInfo) throws XmlPullParserException, IOException {
        return NvConnectionLaunch.launchOrResume(this, h, context, details, app, serverInfo);
    }

    protected boolean quitAndLaunch(NvHTTP h, ConnectionContext context)
            throws IOException, XmlPullParserException {
        return NvConnectionLaunch.quitAndLaunch(this, h, context);
    }

    static final class LocalLaunchFailure extends HostHttpResponseException {
        private static final long serialVersionUID = 1L;
        final int messageId;

        LocalLaunchFailure(int errorCode, int messageId, String logMessage) {
            super(errorCode, logMessage);
            this.messageId = messageId;
        }
    }

    static String launchFailureMessage(Context context, HostHttpResponseException e) {
        return NvConnectionLaunch.launchFailureMessage(context, e);
    }

    // --- Microphone & Controller Audio ---

    boolean controllerMicrophoneOwnsRoute() { return NvConnectionAudio.controllerMicrophoneOwnsRoute(this); }
    synchronized boolean microphoneCapturePermitted() { return NvConnectionAudio.microphoneCapturePermitted(this); }
    void startMicrophone() { NvConnectionAudio.startMicrophone(this, false); }
    static int audioPassthroughCodecs(boolean enabled, boolean muted, int channels) {
        return NvConnectionAudio.audioPassthroughCodecs(enabled, muted, channels);
    }

    public boolean controllerMicrophonePermitted() {
        return canSendControllerInput() && microphoneCapturePermitted();
    }

    public boolean beginControllerMicrophone() { return NvConnectionAudio.beginControllerMicrophone(this); }
    public void endControllerMicrophone() { NvConnectionAudio.endControllerMicrophone(this); }
    public boolean controllerMicrophoneEnabled() { return controllerMicrophonePermitted() && !micMuted; }

    public void controllerMicrophoneState(Object owner, boolean allowed, boolean ready, boolean hostMuted) {
        NvConnectionAudio.updateControllerMicrophone(
                this, owner, new ControllerMicrophoneState(allowed, ready, hostMuted));
    }

    public void releaseControllerMicrophone(Object owner) {
        NvConnectionAudio.updateControllerMicrophone(this, owner, null);
    }

    public void setControllerPreparation(Supplier<String> preparation) { controllerPreparation = preparation; }

    static boolean microphoneWanted(PreferenceConfiguration prefs, NvApp app) {
        return NvConnectionAudio.microphoneWanted(prefs, app);
    }

    void readMicrophoneSettings(PreferenceConfiguration prefs) {
        micEnabled = NvConnectionAudio.microphoneWanted(prefs, context.streamConfig.getApp());
        micPermitted = micEnabled && NvConnectionAudio.hasMicrophonePermission(appContext);
        micProcessing = NvConnectionAudio.microphoneProcessing(
                prefs.micProcessing(), context.streamConfig.getApp());
    }

    static MicProcessing microphoneProcessing(MicProcessing chosen, NvApp app) {
        return NvConnectionAudio.microphoneProcessing(chosen, app);
    }

    MicrophoneCapture createMicrophoneCapture(boolean muted, MicrophoneCapture.FailureListener failed) {
        return NvConnectionAudio.createMicrophoneCapture(this, muted, failed);
    }

    void noteMissingMicrophoneEffects(List<MicProcessing.Effect> effects) {
        micNotice.set(effects);
        Runnable listener = micNoticeListener;
        if (listener != null) listener.run();
    }

    public void setMicrophoneNoticeListener(Runnable listener) {
        micNoticeListener = listener;
        if (listener != null && (micNotice.get() != null || micFailed.get())) listener.run();
    }

    public List<MicProcessing.Effect> takeMicrophoneNotice() { return micNotice.getAndSet(null); }
    public boolean takeMicrophoneFailure() { return micFailed.getAndSet(false); }
    public synchronized MicrophoneStatus getMicrophoneStatus() { return NvConnectionAudio.getMicrophoneStatus(this); }

    public void setMicrophoneMuted(boolean muted) {
        synchronized (this) {
            micMuted = muted;
            if (microphone != null) microphone.setMuted(muted);
        }
        if (!muted && nativeConnected) NvConnectionAudio.startMicrophone(this, true);
    }

    public boolean isMicrophoneMuted() { return micMuted; }
    public synchronized boolean isMicrophoneCapturing() { return microphone != null && microphone.isCapturing(); }
    public synchronized boolean isMicrophoneMutedForReconnect() { return NvConnectionAudio.isMicrophoneMutedForReconnect(this); }

    // --- Input Polling & Permissions ---

    public synchronized void setInputPollingRate(float panelHz) {
        long interval = InputBatcher.intervalNs(panelHz);
        if (interval == inputIntervalNs) return;
        inputIntervalNs = interval;
        if (inputPoller != null) {
            inputPoll.cancel(false);
            inputPoll = inputPoller.scheduleWithFixedDelay(
                    inputBatcher::flush, 0, inputIntervalNs, TimeUnit.NANOSECONDS);
        }
    }

    synchronized void startInputPolling(boolean unbatched) {
        if (stopRequested || unbatched) return;
        inputPoller = Executors.newSingleThreadScheduledExecutor(r -> new Thread(r, "Input - Poll"));
        inputPoll = inputPoller.scheduleWithFixedDelay(
                inputBatcher::flush, 0, inputIntervalNs, TimeUnit.NANOSECONDS);
    }

    public boolean canSendInput() { return (NvConnectionActions.streamPermissionBits(this, hostDetails) & 1) != 0; }
    public boolean canSendControllerInput() { return canSendInput() && (NvConnectionActions.streamPermissionBits(this, hostDetails) & 2) != 0; }

    // --- Host Actions & Native API ---

    public ComputerDetails refreshHostDetails() throws IOException, XmlPullParserException {
        return NvConnectionActions.refreshHostDetails(this);
    }

    public int hostHttpsPort() { return context.httpsPort; }
    public boolean hasEndedSession() { return NvConnectionActions.hasEndedSession(this); }
    public String hostFailureReason() { return NvConnectionActions.hostFailureReason(this); }
    public int startFoundationAbr(int maxKbps) throws IOException { return NvConnectionActions.startFoundationAbr(this, maxKbps); }
    public void stopFoundationAbr() throws IOException { NvConnectionActions.stopFoundationAbr(this); }
    public JSONObject sendFoundationAbrFeedback(JSONObject feedback) throws IOException { return http.sendFoundationAbrFeedback(feedback); }
    public String getClipboard() throws IOException, XmlPullParserException { return NvConnectionActions.getClipboard(this); }
    public boolean getClipboardImage(OutputStream out) throws IOException, XmlPullParserException { return NvConnectionActions.getClipboardImage(this, out); }
    public void quitApp() throws IOException, XmlPullParserException { NvConnectionActions.quitApp(this); }
    public NativeSession nativeSessionAction(NativeSession.ActionId action) throws IOException, XmlPullParserException { return NvConnectionActions.nativeSessionAction(this, action); }
    public boolean hasNativeSessionActions() { return hostDetails.caps.has(HostCapabilities.Feature.NATIVE_API); }
    public void closeGame() throws IOException, XmlPullParserException { NvConnectionActions.closeGame(this); }
    public boolean hasNativeHostControl() { return nativeHostControl; }
    public boolean hasNativeCouchMenu() { return nativeCouchMenu; }

    public boolean offersHostAudio(String id) {
        if (NativeCapabilities.NIGHT_MODE.equals(id)) return nativeNightMode;
        if (NativeCapabilities.HEADPHONE_SURROUND.equals(id)) return nativeHeadphoneSurround;
        return false;
    }

    public boolean offersUsbForwarding() { return nativeUsbForwarding; }
    public boolean askedForClipboardSync() { return context.hostCaps != null && NvHTTP.asksClipboardSync(context); }
    public boolean hasImageClipboard() { return nativeImageClipboard; }
    public boolean offersSessionGuests() { return nativeSessionGuests; }
    public boolean offersQuickSuspend() { return nativeQuickSuspend; }
    public void openHostMenu() throws IOException { NvConnectionActions.openHostMenu(this); }
    public NativeAudio hostAudio() throws IOException { return NvConnectionActions.hostAudio(this); }
    public NativeAudio writeHostAudio(String id, boolean on) throws IOException { return NvConnectionActions.writeHostAudio(this, id, on); }
    public void refreshNativeHostControl() { refreshNativeHostControl(null); }
    public void refreshNativeHostControl(Runnable read) { NvConnectionActions.refreshNativeHostControl(this, read); }

    public NativeApiClient nativeApiClient() {
        if (isMonkey || context.streamConfig.getApp().getRole() != NvApp.Role.STREAM || !hasNativeSessionActions()) return null;
        return NvConnectionActions.nativeClient(this);
    }

    public static String movedMessage(Context context, String device) {
        return device.isEmpty()
                ? context.getString(R.string.continue_here_now_playing_elsewhere)
                : context.getString(R.string.continue_here_now_playing, device);
    }

    public boolean offersContinueHere() { return NativeCapabilities.offersContinueHere(KnownCapabilities.of(hostDetails.uuid)); }

    public String streamMovedTo(boolean recoveryFollows) {
        try {
            return NvConnectionActions.readStreamMovedTo(this, recoveryFollows);
        } catch (IOException e) {
            LimeLog.info(e.getMessage());
            return null;
        }
    }

    public HostEndpoint hostEndpoint() { return NvConnectionActions.hostEndpoint(this); }
    public void announceDisconnect() { NvConnectionActions.announceDisconnect(this); }
    public void setNativeApiTransport(NativeApiTransport transport) { nativeApiTransport = transport; }
    public void disconnectRemoteSession() throws IOException, XmlPullParserException { NvConnectionActions.disconnectRemoteSession(this); }
    public int setBitrate(int kbps) throws IOException, XmlPullParserException { return NvConnectionActions.setBitrate(this, kbps); }
    public void sendClipboard(String text) throws IOException, XmlPullParserException { NvConnectionActions.sendClipboard(this, text); }
    public void sendClipboardImage(File png) throws IOException, XmlPullParserException { NvConnectionActions.sendClipboardImage(this, png); }
    public boolean sendServerCommand(int index, String name) throws IOException, XmlPullParserException { return NvConnectionActions.sendServerCommand(this, index, name); }

    // --- Input Forwarding ---

    public void sendMouseMove(final short deltaX, final short deltaY) { NvConnectionInput.sendMouseMove(this, deltaX, deltaY); }
    public void sendMousePosition(short x, short y, short rw, short rh) { NvConnectionInput.sendMousePosition(this, x, y, rw, rh); }
    public void sendMouseMoveAsMousePosition(short dx, short dy, short rw, short rh) { NvConnectionInput.sendMouseMoveAsMousePosition(this, dx, dy, rw, rh); }
    public void sendMouseButtonDown(final byte mouseButton) { NvConnectionInput.sendOwnedMouseButton(this, mouseButton, true, MouseButtonOwnership.ORDINARY); }
    public void sendMouseButtonUp(final byte mouseButton) { NvConnectionInput.sendOwnedMouseButton(this, mouseButton, false, MouseButtonOwnership.ORDINARY); }
    public void sendOverlayMouseButton(byte mouseButton, boolean down) { NvConnectionInput.sendOwnedMouseButton(this, mouseButton, down, MouseButtonOwnership.GAME_CONTROLS); }
    public void sendDesktopMouseButton(byte mouseButton, boolean down) { NvConnectionInput.sendOwnedMouseButton(this, mouseButton, down, MouseButtonOwnership.DESKTOP_CONTROLS); }

    public void sendControllerInput(final short n, final short mask, final int flags, final byte lt,
            final byte rt, final short lx, final short ly, final short rx, final short ry) {
        NvConnectionInput.sendControllerInput(this, n, mask, flags, lt, rt, lx, ly, rx, ry);
    }

    public void sendKeyboardInput(final short keyMap, final byte dir, final byte mod, final byte flags) { NvConnectionInput.sendKeyboardInput(this, keyMap, dir, mod, flags); }
    public void sendMouseScroll(final byte clicks) { NvConnectionInput.sendMouseHighResScroll(this, (short) (clicks * 120)); }
    public void sendMouseHScroll(final byte clicks) { NvConnectionInput.sendMouseHighResHScroll(this, (short) (clicks * 120)); }
    public void sendMouseHighResScroll(final short amount) { NvConnectionInput.sendMouseHighResScroll(this, amount); }
    public void sendMouseHighResHScroll(final short amount) { NvConnectionInput.sendMouseHighResHScroll(this, amount); }
    public void sendUtf8Text(final String text) { NvConnectionInput.sendUtf8Text(this, text); }

    public int sendTouchEvent(byte type, int id, float x, float y, float p, float major, float minor, short rot) {
        return canSendInput() ? MoonBridge.sendTouchEvent(type, id, x, y, p, major, minor, rot) : MoonBridge.LI_ERR_UNSUPPORTED;
    }

    public int touchpadTransport() { return canSendInput() ? MoonBridge.touchpadTransport() : MoonBridge.TOUCHPAD_NONE; }

    public int sendTouchpadFrame(int dev, int[] ids, float[] xs, float[] ys, float[] ps, int count, boolean btn, short wMm, short hMm) {
        return canSendInput() ? MoonBridge.touchpadFrame(dev, ids, xs, ys, ps, count, btn, wMm, hMm) : MoonBridge.LI_ERR_UNSUPPORTED;
    }

    public int cancelTouchpad() { return canSendInput() ? MoonBridge.touchpadCancel() : MoonBridge.LI_ERR_UNSUPPORTED; }

    public int sendPenEvent(byte et, byte tt, byte btn, float x, float y, float p, float maj, float min, short rot, byte tilt) {
        return canSendInput() ? MoonBridge.sendPenEvent(et, tt, btn, x, y, p, maj, min, rot, tilt) : MoonBridge.LI_ERR_UNSUPPORTED;
    }

    public int sendControllerIdentityEvent(byte n, int vid, int pid) {
        if (vid == 0 && pid == 0) return 0;
        return canSendControllerInput() ? MoonBridge.sendControllerIdentityEvent(n, vid, pid) : MoonBridge.LI_ERR_UNSUPPORTED;
    }

    public int sendControllerArrivalEvent(byte n, short mask, byte type, int btnFlags, short caps) {
        return canSendControllerInput() ? MoonBridge.sendControllerArrivalEvent(n, mask, type, btnFlags, caps) : MoonBridge.LI_ERR_UNSUPPORTED;
    }

    public int sendControllerTouchEvent(byte n, byte et, int id, float x, float y, float p) {
        return canSendControllerInput() ? MoonBridge.sendControllerTouchEvent(n, et, id, x, y, p) : MoonBridge.LI_ERR_UNSUPPORTED;
    }

    public int sendControllerMotionEvent(byte n, byte mt, float x, float y, float z) {
        return canSendControllerInput() ? MoonBridge.sendControllerMotionEvent(n, mt, x, y, z) : MoonBridge.LI_ERR_UNSUPPORTED;
    }

    public void sendControllerBatteryEvent(byte n, byte state, byte pct) {
        if (canSendControllerInput()) MoonBridge.sendControllerBatteryEvent(n, state, pct);
    }

    public static String findExternalAddressForMdns(String stunHostname, int stunPort) {
        return MoonBridge.findExternalAddressIP4(stunHostname, stunPort);
    }
}
