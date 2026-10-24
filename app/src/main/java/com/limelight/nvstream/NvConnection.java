package com.limelight.nvstream;

import android.Manifest;
import android.app.ActivityManager;
import android.content.Context;
import android.content.pm.PackageManager;
import android.net.ConnectivityManager;
import android.net.IpPrefix;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.NetworkInfo;
import android.net.RouteInfo;
import android.os.Build;

import java.io.FileNotFoundException;
import java.io.IOException;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.security.NoSuchAlgorithmException;
import java.security.SecureRandom;
import java.security.cert.X509Certificate;
import java.util.Timer;
import java.util.TimerTask;
import java.util.concurrent.Semaphore;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;
import java.util.function.Consumer;

import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;

import org.xmlpull.v1.XmlPullParserException;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.audio.MicrophoneCapture;
import com.limelight.binding.audio.MicrophoneStatus;
import com.limelight.binding.input.InputBatcher;
import com.limelight.binding.video.Sdr10Policy;
import com.limelight.computers.HostNotices;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.JniNativeApiTransport;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativeApiTransport;
import com.limelight.nativeapi.NativeSession;
import com.limelight.nvstream.av.audio.AudioRenderer;
import com.limelight.nvstream.av.video.VideoDecoderRenderer;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HardwareMetrics;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.HostHttpResponseException;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import com.limelight.nvstream.http.LaunchConfirmation;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.nvstream.input.MouseButtonPacket;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.HostDisplayChoice;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.utils.NetworkGrace;
import com.limelight.utils.PyroWavePaceStore;
import com.limelight.utils.WiredLink;

public class NvConnection {
    private static final class SessionEndedException extends IOException {}
    // Context parameters
    private LimelightCryptoProvider cryptoProvider;
    private String uniqueId;
    private ConnectionContext context;
    private NativeApiTransport nativeApiTransport = new JniNativeApiTransport();
    private volatile boolean nativeHostControl;
    private static Semaphore connectionAllowed = new Semaphore(1);
    private final boolean isMonkey;
    private final Context appContext;
    private volatile ComputerDetails hostDetails = new ComputerDetails();
    private NvHTTP http;
    private Thread connectionThread;
    private volatile boolean stopRequested;
    private boolean nativeStarted;
    private volatile boolean nativeConnected;
    private long lastServerCommandTime;
    private volatile ScheduledExecutorService inputPoller;
    private ScheduledFuture<?> inputPoll;
    private ScheduledExecutorService hardwareMetricsPoller;
    private NvHTTP hardwareMetricsHttp;
    private boolean hardwareMetricsUnavailable;
    private long inputIntervalNs = InputBatcher.intervalNs(60);
    // Microphone: whether the setting was on and RECORD_AUDIO granted when this connection
    // started, the capture thread while one runs, and the user's mute choice.
    private volatile boolean micEnabled;
    private volatile boolean micPermitted;
    private volatile boolean micMuted;
    private MicrophoneCapture microphone;
    private static final class ControllerMicrophoneState {
        final boolean allowed, ready, hostMuted;
        ControllerMicrophoneState(boolean allowed, boolean ready, boolean hostMuted) {
            this.allowed = allowed;
            this.ready = allowed && ready;
            // Losing the host capture endpoint must not unmute phone fallback.
            this.hostMuted = hostMuted;
        }
    }
    private final java.util.Map<Object, ControllerMicrophoneState> controllerMicrophones =
            new java.util.IdentityHashMap<>();
    private long microphoneAccessAt;
    private boolean microphoneAccess;
    // Ordinary fallback shares Android microphone policy, but does not require
    // permission to create or send input from a host controller.
    synchronized boolean microphoneCapturePermitted() {
        if (!micPermitted || !canSendInput()) return false;
        long now = android.os.SystemClock.uptimeMillis();
        if (microphoneAccessAt != 0 && now - microphoneAccessAt < 100) return microphoneAccess;
        android.media.AudioManager audio = (android.media.AudioManager)appContext.getSystemService(Context.AUDIO_SERVICE);
        int mode = recordAudioOp(RECORD_AUDIO_CHECK);
        microphoneAccess = hasMicrophonePermission() && audio != null && !audio.isMicrophoneMute() &&
                (mode == android.app.AppOpsManager.MODE_ALLOWED || mode == android.app.AppOpsManager.MODE_DEFAULT);
        microphoneAccessAt = now;
        return microphoneAccess;
    }
    private static final int RECORD_AUDIO_CHECK = 0, RECORD_AUDIO_START = 1, RECORD_AUDIO_FINISH = 2;
    // The record-audio op string only exists from Android 6.0; looking it up on 5.x throws
    // IllegalArgumentException. Those releases grant RECORD_AUDIO at install, so there is nothing to ask.
    private int recordAudioOp(int action) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return android.app.AppOpsManager.MODE_ALLOWED;
        android.app.AppOpsManager ops = (android.app.AppOpsManager)appContext.getSystemService(Context.APP_OPS_SERVICE);
        if (ops == null) return android.app.AppOpsManager.MODE_ERRORED;
        int uid = android.os.Process.myUid();
        String op = android.app.AppOpsManager.OPSTR_RECORD_AUDIO, packageName = appContext.getPackageName();
        try {
            if (action == RECORD_AUDIO_CHECK) return ops.checkOpNoThrow(op, uid, packageName);
            if (action == RECORD_AUDIO_START) return ops.startOpNoThrow(op, uid, packageName);
            ops.finishOp(op, uid, packageName);
            return android.app.AppOpsManager.MODE_ALLOWED;
        } catch (IllegalArgumentException | SecurityException e) {
            LimeLog.warning("Microphone app-op unavailable: " + e);
            return android.app.AppOpsManager.MODE_ERRORED;
        }
    }
    public boolean controllerMicrophonePermitted() {
        return canSendControllerInput() && microphoneCapturePermitted();
    }
    public boolean beginControllerMicrophone() {
        if (!controllerMicrophoneEnabled()) return false;
        return recordAudioOp(RECORD_AUDIO_START) == android.app.AppOpsManager.MODE_ALLOWED;
    }
    public void endControllerMicrophone() {
        recordAudioOp(RECORD_AUDIO_FINISH);
    }
    public boolean controllerMicrophoneEnabled() { return controllerMicrophonePermitted() && !micMuted; }
    public void controllerMicrophoneState(Object owner, boolean allowed, boolean ready, boolean hostMuted) {
        updateControllerMicrophone(owner, new ControllerMicrophoneState(allowed, ready, hostMuted));
    }
    public void releaseControllerMicrophone(Object owner) {
        updateControllerMicrophone(owner, null);
    }
    private void updateControllerMicrophone(Object owner, ControllerMicrophoneState state) {
        if (owner == null) return;
        MicrophoneCapture previousCapture = null;
        boolean fallback;
        synchronized (this) {
            if (!canSendControllerInput()) state = null;
            ControllerMicrophoneState previous = controllerMicrophones.get(owner);
            // Removing a muted device must not turn another microphone on. The
            // user can unmute the fallback through the existing stream control.
            if (state == null && previous != null && previous.hostMuted) micMuted = true;
            if (state == null) controllerMicrophones.remove(owner);
            else controllerMicrophones.put(owner, state);
            if (controllerMicrophoneOwnsRoute()) {
                previousCapture = microphone;
                microphone = null;
            }
            fallback = !controllerMicrophoneOwnsRoute() && microphone == null && nativeConnected && !stopRequested;
        }
        // Do not wait for the native connection lock from a USB worker: teardown
        // joins that worker before interrupting a connection still being started.
        if (previousCapture != null) previousCapture.stop();
        if (fallback) startMicrophone(true);
    }
    private boolean controllerMicrophoneOwnsRoute() {
        for (ControllerMicrophoneState state : controllerMicrophones.values()) {
            if (state.ready || state.hostMuted) return true;
        }
        return false;
    }
    private java.util.function.Supplier<String> controllerPreparation;
    public void setControllerPreparation(java.util.function.Supplier<String> preparation) {
        controllerPreparation = preparation;
    }
    private final InputBatcher inputBatcher = new InputBatcher(motion -> {
        if (!canSendInput()) return;
        if (motion.kind == 0) MoonBridge.sendMouseMove((short) motion.x, (short) motion.y);
        else if (motion.kind == 1) MoonBridge.sendMousePosition((short) motion.x, (short) motion.y, motion.width, motion.height);
        else MoonBridge.sendMouseMoveAsMousePosition((short) motion.x, (short) motion.y, motion.width, motion.height);
    });

    public synchronized void setInputPollingRate(float panelHz) {
        long interval = InputBatcher.intervalNs(panelHz);
        if (interval == inputIntervalNs) return;
        inputIntervalNs = interval;
        if (inputPoller != null) {
            inputPoll.cancel(false);
            inputPoll = inputPoller.scheduleWithFixedDelay(inputBatcher::flush, 0, inputIntervalNs, TimeUnit.NANOSECONDS);
        }
    }

    private synchronized void startInputPolling(boolean unbatched) {
        if (stopRequested || unbatched) return;
        inputPoller = Executors.newSingleThreadScheduledExecutor(r -> new Thread(r, "Input - Poll"));
        inputPoll = inputPoller.scheduleWithFixedDelay(inputBatcher::flush, 0, inputIntervalNs, TimeUnit.NANOSECONDS);
    }

    private boolean hasMicrophonePermission() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) {
            return true;
        }
        return appContext.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED;
    }

    // Starts only once the native connection is ready; the final lock check also
    // serializes fallback creation with connection stop and a newly ready USB mic.
    private void startMicrophone() {
        startMicrophone(false);
    }
    private void startMicrophone(boolean fallback) {
        synchronized (this) {
            if (!micPermitted || stopRequested || controllerMicrophoneOwnsRoute() ||
                    (fallback && (micMuted || !microphoneCapturePermitted()))) return;
        }
        int state = MoonBridge.getMicrophoneState();
        if (state != MoonBridge.MIC_STATE_READY) {
            if (!fallback) LimeLog.info("Microphone: not sending (" + (state == MoonBridge.MIC_STATE_NOT_OFFERED ?
                    "not offered by host" : "host did not set it up") + ")");
            return;
        }
        synchronized (this) {
            if (stopRequested || controllerMicrophoneOwnsRoute() || microphone != null ||
                    (fallback && (micMuted || !microphoneCapturePermitted()))) {
                return;
            }
            microphone = createMicrophoneCapture(micMuted);
            microphone.start();
        }
    }

    // Package-visible factory keeps route transitions testable without an audio device.
    MicrophoneCapture createMicrophoneCapture(boolean muted) {
        return new MicrophoneCapture(MoonBridge::sendMicrophonePcm, muted);
    }

    private void stopMicrophone() {
        MicrophoneCapture capture;
        synchronized (this) {
            capture = microphone;
            microphone = null;
        }
        if (capture != null) {
            capture.stop();
        }
    }

    public synchronized MicrophoneStatus getMicrophoneStatus() {
        boolean controllerMuted = false;
        for (ControllerMicrophoneState state : controllerMicrophones.values()) {
            if (state.ready) {
                return MicrophoneStatus.of(micEnabled, micPermitted, MoonBridge.MIC_STATE_READY,
                        true, micMuted);
            }
            controllerMuted |= state.hostMuted || (state.allowed && micMuted);
        }
        if (controllerMuted) return MicrophoneStatus.of(micEnabled, micPermitted,
                MoonBridge.MIC_STATE_READY, true, true);
        return MicrophoneStatus.of(micEnabled, micPermitted, micPermitted ? MoonBridge.getMicrophoneState() :
                MoonBridge.MIC_STATE_OFF, micMuted || (microphone != null && microphone.isCapturing()), micMuted);
    }

    public void setMicrophoneMuted(boolean muted) {
        synchronized (this) {
            micMuted = muted;
            if (microphone != null) microphone.setMuted(muted);
        }
        if (!muted && nativeConnected) startMicrophone(true);
    }

    public boolean isMicrophoneMuted() {
        return micMuted;
    }

    // Snapshot before teardown clears controller ownership. This must not query
    // native state, which may already belong to a different connection.
    public synchronized boolean isMicrophoneMutedForReconnect() {
        if (micMuted) return true;
        for (ControllerMicrophoneState state : controllerMicrophones.values()) {
            if (state.hostMuted) return true;
        }
        return false;
    }

    public NvConnection(Context appContext, ComputerDetails.AddressTuple host, int httpsPort, String uniqueId, StreamConfiguration config, LimelightCryptoProvider cryptoProvider, X509Certificate serverCert)
    {
        this.appContext = appContext;
        this.cryptoProvider = cryptoProvider;
        this.uniqueId = uniqueId;

        this.context = new ConnectionContext();
        this.context.serverAddress = host;
        this.context.httpsPort = httpsPort;
        this.context.streamConfig = config;
        this.context.serverCert = serverCert;

        // This is unique per connection
        this.context.riKey = generateRiAesKey();
        this.context.riKeyId = generateRiKeyId();

        this.isMonkey = ActivityManager.isUserAMonkey();
    }
    
    private static SecretKey generateRiAesKey() {
        try {
            KeyGenerator keyGen = KeyGenerator.getInstance("AES");

            // RI keys are 128 bits
            keyGen.init(128);

            return keyGen.generateKey();
        } catch (NoSuchAlgorithmException e) {
            e.printStackTrace();
            throw new RuntimeException(e);
        }
    }
    
    private static int generateRiKeyId() {
        return new SecureRandom().nextInt();
    }

    public void stop() {
        synchronized (this) {
            if (stopRequested) {
                return;
            }
            stopRequested = true;
            controllerMicrophones.clear();
            stopHardwareMetricsPolling();
            if (inputPoller != null) {
                inputPoller.shutdownNow();
                inputPoller = null;
            }
            inputBatcher.clear();
            if (connectionThread != null && !nativeStarted) {
                connectionThread.interrupt();
            }
            if (http != null) {
                http.cancelPendingRequests();
            }
            // A cancelled HTTP request or semaphore waiter does not own the global native connection.
            if (nativeStarted) {
                MoonBridge.interruptConnection();
            }
        }

        // Moonlight-core is not thread-safe with respect to connection start and stop, so
        // we must not invoke that functionality in parallel.
        synchronized (MoonBridge.class) {
            // Stop recording before the native side closes the microphone socket.
            stopMicrophone();
            if (nativeConnected) {
                nativeConnected = false;
                MoonBridge.stopConnection();
                MoonBridge.cleanupBridge();
                synchronized (this) {
                    nativeStarted = false;
                }
                connectionAllowed.release();
            }
        }
    }

    private InetAddress resolveServerAddress() throws IOException {
        // Try to find an address that works for this host
        InetAddress[] addrs = InetAddress.getAllByName(context.serverAddress.address);
        for (InetAddress addr : addrs) {
            try (Socket s = new Socket()) {
                s.setSoLinger(true, 0);
                s.connect(new InetSocketAddress(addr, context.serverAddress.port), 1000);
                return addr;
            } catch (IOException e) {
                e.printStackTrace();
            }
        }

        // If we made it here, we didn't manage to find a working address. If DNS returned any
        // address, we'll use the first available address and hope for the best.
        if (addrs.length > 0) {
            return addrs[0];
        }
        else {
            throw new IOException("No addresses found for "+context.serverAddress);
        }
    }

    private int detectServerConnectionType() {
        ConnectivityManager connMgr = (ConnectivityManager) appContext.getSystemService(Context.CONNECTIVITY_SERVICE);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            Network activeNetwork = connMgr.getActiveNetwork();
            if (activeNetwork != null) {
                NetworkCapabilities netCaps = connMgr.getNetworkCapabilities(activeNetwork);
                if (netCaps != null) {
                    if (netCaps.hasTransport(NetworkCapabilities.TRANSPORT_VPN) ||
                            !netCaps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)) {
                        // VPNs are treated as remote connections
                        return StreamConfiguration.STREAM_CFG_REMOTE;
                    }
                    else if (netCaps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR)) {
                        // Cellular is always treated as remote to avoid any possible
                        // issues with 464XLAT or similar technologies.
                        return StreamConfiguration.STREAM_CFG_REMOTE;
                    }
                }

                // Check if the server address is on-link
                LinkProperties linkProperties = connMgr.getLinkProperties(activeNetwork);
                if (linkProperties != null) {
                    InetAddress serverAddress;
                    try {
                        serverAddress = resolveServerAddress();
                    } catch (IOException e) {
                        e.printStackTrace();

                        // We can't decide without being able to resolve the server address
                        return StreamConfiguration.STREAM_CFG_AUTO;
                    }

                    // If the address is in the NAT64 prefix, always treat it as remote
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                        IpPrefix nat64Prefix = linkProperties.getNat64Prefix();
                        if (nat64Prefix != null && nat64Prefix.contains(serverAddress)) {
                            return StreamConfiguration.STREAM_CFG_REMOTE;
                        }
                    }

                    for (RouteInfo route : linkProperties.getRoutes()) {
                        // Skip non-unicast routes (which are all we get prior to Android 13)
                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU && route.getType() != RouteInfo.RTN_UNICAST) {
                            continue;
                        }

                        // Find the first route that matches this address
                        if (route.matches(serverAddress)) {
                            // If there's no gateway, this is an on-link destination
                            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                                // We want to use hasGateway() because getGateway() doesn't adhere
                                // to documented behavior of returning null for on-link addresses.
                                if (!route.hasGateway()) {
                                    return StreamConfiguration.STREAM_CFG_LOCAL;
                                }
                            }
                            else {
                                // getGateway() is documented to return null for on-link destinations,
                                // but it actually returns the unspecified address (0.0.0.0 or ::).
                                InetAddress gateway = route.getGateway();
                                if (gateway == null || gateway.isAnyLocalAddress()) {
                                    return StreamConfiguration.STREAM_CFG_LOCAL;
                                }
                            }

                            // We _should_ stop after the first matching route, but for some reason
                            // Android doesn't always report IPv6 routes in descending order of
                            // specificity and metric. To handle that case, we enumerate all matching
                            // routes, assuming that an on-link route will always be preferred.
                        }
                    }
                }
            }
        }
        else {
            NetworkInfo activeNetworkInfo = connMgr.getActiveNetworkInfo();
            if (activeNetworkInfo != null) {
                switch (activeNetworkInfo.getType()) {
                    case ConnectivityManager.TYPE_VPN:
                    case ConnectivityManager.TYPE_MOBILE:
                    case ConnectivityManager.TYPE_MOBILE_DUN:
                    case ConnectivityManager.TYPE_MOBILE_HIPRI:
                    case ConnectivityManager.TYPE_MOBILE_MMS:
                    case ConnectivityManager.TYPE_MOBILE_SUPL:
                    case ConnectivityManager.TYPE_WIMAX:
                        // VPNs and cellular connections are always remote connections
                        return StreamConfiguration.STREAM_CFG_REMOTE;
                }
            }
        }

        // If we can't determine the connection type, let moonlight-common-c decide.
        return StreamConfiguration.STREAM_CFG_AUTO;
    }
    
    /**
     * HDR is streaming, but not as PyroWave although the host and this device both offered
     * PyroWave 10-bit: the device could not create its HDR output.
     */
    static boolean pyroWaveHdrLost(int commonFormats, int negotiatedFormats) {
        int pyroWaveHdr = MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE & MoonBridge.VIDEO_FORMAT_MASK_10BIT;
        return (commonFormats & pyroWaveHdr) != 0 && (negotiatedFormats & pyroWaveHdr) == 0 &&
                (negotiatedFormats & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0;
    }

    static int negotiateVideoFormats(int formats, int serverFormats) {
        formats &= MoonBridge.VIDEO_FORMAT_MASK_H264 | MoonBridge.VIDEO_FORMAT_MASK_H265 |
                MoonBridge.VIDEO_FORMAT_MASK_AV1 | MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE;

        int[] clientBits = { MoonBridge.VIDEO_FORMAT_H265_MAIN10, MoonBridge.VIDEO_FORMAT_AV1_MAIN10,
                MoonBridge.VIDEO_FORMAT_H264_HIGH8_444, MoonBridge.VIDEO_FORMAT_H265_REXT8_444,
                MoonBridge.VIDEO_FORMAT_H265_REXT10_444, MoonBridge.VIDEO_FORMAT_AV1_HIGH8_444,
                MoonBridge.VIDEO_FORMAT_AV1_HIGH10_444, MoonBridge.VIDEO_FORMAT_PYROWAVE,
                MoonBridge.VIDEO_FORMAT_PYROWAVE_444, MoonBridge.VIDEO_FORMAT_PYROWAVE_MAIN10,
                MoonBridge.VIDEO_FORMAT_PYROWAVE_MAIN10_444 };
        int[] hostBits = { 0x200, 0x20000, 0x40000, 0x80000, 0x100000, 0x200000, 0x400000, 0x800000, 0x1000000, 0x2000000, 0x4000000 };
        for (int i = 0; i < clientBits.length; i++) {
            if ((serverFormats & hostBits[i]) == 0) {
                formats &= ~clientBits[i];
            }
        }

        int pyroWave = formats & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE;
        formats &= ~MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE;
        // HDR takes precedence over an SDR-only PyroWave host.
        if ((formats & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0) {
            pyroWave &= MoonBridge.VIDEO_FORMAT_MASK_10BIT;
        }

        // The native handshake prefers AV1 over HEVC regardless of bit depth/chroma.
        // Keep it from choosing SDR or 4:2:0 ahead of an available requested format.
        int preferred = (formats & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0 ?
                MoonBridge.VIDEO_FORMAT_MASK_10BIT : MoonBridge.VIDEO_FORMAT_MASK_YUV444;
        if ((formats & preferred) != 0) {
            if ((formats & MoonBridge.VIDEO_FORMAT_MASK_AV1 & preferred) == 0) {
                formats &= ~MoonBridge.VIDEO_FORMAT_MASK_AV1;
            }
            if ((formats & MoonBridge.VIDEO_FORMAT_MASK_H265 & preferred) == 0) {
                formats &= ~MoonBridge.VIDEO_FORMAT_MASK_H265;
            }
        }
        return formats | pyroWave;
    }

    private boolean startApp(VideoDecoderRenderer renderer) throws XmlPullParserException, IOException
    {
        NvHTTP h = new NvHTTP(context.serverAddress, context.httpsPort, uniqueId, context.serverCert, cryptoProvider);
        synchronized (this) {
            http = h;
            if (stopRequested) {
                h.cancelPendingRequests();
            }
        }

        String serverInfo = h.getServerInfo(true);
        
        context.serverAppVersion = h.getServerVersion(serverInfo);
        if (context.serverAppVersion == null) {
            context.connListener.displayMessage(appContext.getString(R.string.startup_invalid_host));
            return false;
        }

        ComputerDetails details = h.getComputerDetails(serverInfo);
        hostDetails = details;
        context.isNvidiaServerSoftware = details.nvidiaServer;
        context.hostCaps = details.caps;
        LimeLog.info("Host: " + details.caps);
        context.streamConfig.getApp().setHostCapabilities(details.caps);
        if (canSendControllerInput() && details.caps.has(HostCapabilities.Feature.CONTROLLER_PREPARATION) &&
                controllerPreparation != null) {
            context.preparedControllers = controllerPreparation.get();
        }
        // The choice saved for the PC this stream was started from (its stored ID); the ID in this
        // serverinfo reply is only the fallback.
        context.hostDisplay = context.streamConfig.getHostDisplay() != null ?
                context.streamConfig.getHostDisplay() : HostDisplayChoice.load(appContext, details.uuid);
        NvApp.Role role = context.streamConfig.getApp().getRole();
        boolean virtualDisplay = details.caps.has(HostCapabilities.Feature.VIRTUAL_DISPLAY);
        LimeLog.info("Display on the PC: " + NvHTTP.launchDisplayChoice(context, role).name() +
                ", host virtual display " + (context.streamConfig.getVirtualDisplay() ? "on" : "off") +
                (virtualDisplay ? "" : " (the PC reports no virtual display)"));
        // A PC without a virtual display stays without one: say so once for it, not every stream.
        if (virtualDisplay) {
            HostNotices.reset(appContext, details.uuid, HostNotices.Notice.VIRTUAL_DISPLAY_UNAVAILABLE);
        } else if (NvHTTP.wantsVirtualDisplay(context, role) &&
                HostNotices.claim(appContext, details.uuid, HostNotices.Notice.VIRTUAL_DISPLAY_UNAVAILABLE)) {
            context.connListener.displayTransientMessage(appContext.getString(R.string.startup_virtual_display_unavailable));
        }

        // May be missing for older servers
        context.serverGfeVersion = h.getGfeVersion(serverInfo);
                
        if (h.getPairState(serverInfo) != PairingManager.PairState.PAIRED) {
            context.connListener.displayMessage(appContext.getString(R.string.startup_not_paired));
            return false;
        }

        NvApp app = context.streamConfig.getApp();
        NvApp currentApp = null;

        if (context.streamConfig.isResumeOnly() &&
                (app.getControl() != NvApp.Control.NONE ||
                        !app.matchesRunningApp(details.runningGameId, details.runningGameUuid))) {
            if (details.runningGameId == 0 && context.streamConfig.isIdleSessionEndAllowed() && hasEndedSession(h)) {
                throw new SessionEndedException();
            }
            context.connListener.displayMessage(appContext.getString(R.string.stream_recovery_session_changed));
            return false;
        }

        if (!app.getAppUuid().isEmpty()) {
            // Resolve stale shortcut IDs before the host's ID-or-UUID lookup can select another app.
            if (details.runningGameId != 0 && app.matchesRunningApp(details.runningGameId, details.runningGameUuid)) {
                app.setAppId(details.runningGameId);
            } else {
                currentApp = details.hasPermission(ComputerDetails.PERMISSION_LIST) ?
                        h.getAppByUuid(app.getAppUuid()) : null;
                // Known UUIDs can still be launched without List permission; control tiles also
                // appear and disappear with ownership. Zero leaves identity resolution to a host
                // that looks the UUID up first. Apollo compares the ID with its running app's
                // before it reads the UUID, so an idle Apollo would take zero as "resume" and
                // stream without starting the app: it keeps the ID the tile was listed with.
                if (currentApp != null) {
                    app.setAppId(currentApp.getAppId());
                    app.setMonitorResume(currentApp.getRole() == NvApp.Role.REMOTE_MONITOR);
                } else if (details.caps.has(HostCapabilities.Feature.LAUNCH_BY_UUID)) {
                    app.setAppId(0);
                }
            }
        }

        // If the client did not provide an exact app ID, do a lookup with the applist
        if (!context.streamConfig.getApp().isInitialized()) {
            LimeLog.info("Using deprecated app lookup method - Please specify an app ID in your StreamConfiguration instead");
            currentApp = h.getAppByName(context.streamConfig.getApp().getAppName());
            if (currentApp == null) {
                context.connListener.displayMessage(appContext.getString(R.string.startup_app_missing));
                return false;
            }
            app.setAppId(currentApp.getAppId());
            app.setAppUuid(currentApp.getAppUuid());
            app.setMonitorResume(currentApp.getRole() == NvApp.Role.REMOTE_MONITOR);
        }

        if (app.getControl() == NvApp.Control.RESUME) {
            if (currentApp == null) {
                currentApp = app.getAppUuid().isEmpty() ? h.getAppById(app.getAppId()) : h.getAppByUuid(app.getAppUuid());
            }
            // Resume changes meaning with host ownership; a stale shortcut cannot determine its role.
            if (currentApp == null) {
                context.connListener.displayMessage(appContext.getString(R.string.startup_resume_missing));
                return false;
            }
            app.setMonitorResume(currentApp.getRole() == NvApp.Role.REMOTE_MONITOR);
        }

        // PyroWave's bits are only trusted from a host that also shows PyroWave; the stream
        // description then still has to offer it.
        context.serverCodecModeSupport = details.caps.trustedCodecModeSupport((int) h.getServerCodecModeSupport(serverInfo));

        //
        // Decide on negotiated stream parameters now
        //
        
        if (app.isSmallStreamInputOnly()) {
            // An ordinary stream to the host at the size the caller chose; this device drops it.
            context.negotiatedWidth = context.streamConfig.getWidth();
            context.negotiatedHeight = context.streamConfig.getHeight();
            context.negotiatedVideoFormats = MoonBridge.VIDEO_FORMAT_H264;
        } else if (app.getRole() == NvApp.Role.INPUT_ONLY || app.isControlAction()) {
            // The host still negotiates RTSP, but input-only never starts media streams.
            context.negotiatedWidth = 1280;
            context.negotiatedHeight = 720;
            context.negotiatedVideoFormats = MoonBridge.VIDEO_FORMAT_H264;
        } else {
            // Check for a supported stream resolution
            if ((context.streamConfig.getWidth() > 4096 || context.streamConfig.getHeight() > 4096) &&
                    (h.getServerCodecModeSupport(serverInfo) & 0x200) == 0 && context.isNvidiaServerSoftware) {
                context.connListener.displayMessage(appContext.getString(R.string.startup_host_resolution_unsupported));
                return false;
            }
            else if ((context.streamConfig.getWidth() > 4096 || context.streamConfig.getHeight() > 4096) &&
                    (context.streamConfig.getSupportedVideoFormats() & ~MoonBridge.VIDEO_FORMAT_MASK_H264) == 0) {
                context.connListener.displayMessage(appContext.getString(R.string.startup_device_resolution_unsupported));
                return false;
            }
            else if (context.streamConfig.getHeight() >= 2160 && !h.supports4K(serverInfo)) {
                // Client wants 4K but the server can't do it: said once for the PC.
                if (HostNotices.claim(appContext, details.uuid, HostNotices.Notice.HOST_4K_UNAVAILABLE)) {
                    context.connListener.displayTransientMessage(appContext.getString(R.string.startup_host_update_4k));
                }
            
                // Lower resolution to 1080p
                context.negotiatedWidth = 1920;
                context.negotiatedHeight = 1080;
            }
            else {
                // Take what the client wanted
                context.negotiatedWidth = context.streamConfig.getWidth();
                context.negotiatedHeight = context.streamConfig.getHeight();
            }

            int offeredFormats = context.streamConfig.getSupportedVideoFormats();
            int commonFormats = negotiateVideoFormats(offeredFormats, context.serverCodecModeSupport);
            // The pick order ranks Main10 above native 4:4:4, so when both are possible with this PC,
            // 4:4:4 is kept by leaving the 10-bit formats out before negotiating.
            boolean yuv444OverSdr10 = context.streamConfig.isSdr10Bit() && Sdr10Policy.yuv444Wins(commonFormats);
            if (yuv444OverSdr10) {
                offeredFormats = Sdr10Policy.withoutTenBit(offeredFormats);
                commonFormats = negotiateVideoFormats(offeredFormats, context.serverCodecModeSupport);
            }
            renderer.setStreamFrameRateMilliHz(NvHTTP.getStreamFrameRateMilliHz(context));
            int preparedFormats = renderer.prepareVideoFormats(commonFormats, context.negotiatedWidth,
                    context.negotiatedHeight, context.streamConfig.getRefreshRate());
            context.negotiatedVideoFormats = negotiateVideoFormats(preparedFormats, context.serverCodecModeSupport);
            context.negotiated10Bit = (context.negotiatedVideoFormats & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0;
            // 10-bit SDR offers 10-bit formats without asking for HDR in the launch.
            context.negotiatedHdr = context.negotiated10Bit && !context.streamConfig.isSdr10Bit();
            if (pyroWaveHdrLost(commonFormats, context.negotiatedVideoFormats)) {
                // The host and this device both offer PyroWave HDR, but its output could not be
                // created here; AV1 or HEVC HDR carries the stream instead. Say so once per PC.
                int reason = renderer.pyroWaveHdrFailureReason();
                LimeLog.warning("PyroWave HDR unavailable, streaming " + Integer.toHexString(context.negotiatedVideoFormats) +
                        (reason != 0 ? ": " + appContext.getString(reason) : ""));
                if (HostNotices.claim(appContext, details.uuid, HostNotices.Notice.PYROWAVE_HDR_UNAVAILABLE)) {
                    context.connListener.displayTransientMessage(appContext.getString(
                            R.string.startup_pyrowave_hdr_unavailable,
                            appContext.getString(reason != 0 ? reason : R.string.pyrowave_check_failed)));
                }
            } else if ((context.negotiatedVideoFormats & MoonBridge.VIDEO_FORMAT_MASK_10BIT &
                    MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
                HostNotices.reset(appContext, details.uuid, HostNotices.Notice.PYROWAVE_HDR_UNAVAILABLE);
            }
            if (yuv444OverSdr10) {
                // No 10-bit was negotiated, so the launch does not say hdrMode=0 either.
                LimeLog.warning("10-bit SDR not used: native 4:4:4 takes precedence");
                if (HostNotices.claim(appContext, details.uuid, HostNotices.Notice.SDR10_UNAVAILABLE)) {
                    context.connListener.displayTransientMessage(appContext.getString(R.string.stream_sdr10_unavailable,
                            appContext.getString(R.string.stream_sdr10_reason_yuv444)));
                }
            } else if (context.streamConfig.isSdr10Bit()) {
                HostNotices.reset(appContext, details.uuid, HostNotices.Notice.SDR10_UNAVAILABLE);
                if (!context.negotiated10Bit) {
                    LimeLog.warning("10-bit SDR offered, but this PC streams 8-bit");
                    if (HostNotices.claim(appContext, details.uuid, HostNotices.Notice.SDR10_NOT_ACCEPTED)) {
                        context.connListener.displayTransientMessage(appContext.getString(R.string.startup_sdr10_not_accepted));
                    }
                } else {
                    // 10-bit works with this PC now; if it stops, say so again.
                    HostNotices.reset(appContext, details.uuid, HostNotices.Notice.SDR10_NOT_ACCEPTED);
                }
            } else if (!context.negotiatedHdr && (offeredFormats & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0) {
                if (HostNotices.claim(appContext, details.uuid, HostNotices.Notice.HDR_UNAVAILABLE)) {
                    context.connListener.displayTransientMessage(appContext.getString(R.string.startup_hdr_unavailable));
                }
            } else if (context.negotiatedHdr) {
                // HDR works with this PC now; if it stops, say so again.
                HostNotices.reset(appContext, details.uuid, HostNotices.Notice.HDR_UNAVAILABLE);
            }

        }

        // We will perform some connection type detection if the caller asked for it
        if (context.streamConfig.getRemote() == StreamConfiguration.STREAM_CFG_AUTO) {
            context.negotiatedRemoteStreaming = detectServerConnectionType();
            context.negotiatedPacketSize =
                    context.negotiatedRemoteStreaming == StreamConfiguration.STREAM_CFG_REMOTE ?
                            1024 : context.streamConfig.getMaxPacketSize();
        }
        else {
            context.negotiatedRemoteStreaming = context.streamConfig.getRemote();
            context.negotiatedPacketSize = context.streamConfig.getMaxPacketSize();
        }
        
        //
        // Video stream format will be decided during the RTSP handshake
        //
        
        // The host also exposes resume/control tiles as apps, so it must decide whether a launch needs launch permission.
        if (!details.hasPermission(ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_LAUNCH)) {
            context.connListener.displayMessage(appContext.getString(R.string.startup_permission_denied));
            return false;
        }

        if (app.getControl() != NvApp.Control.NONE) {
            return launchNotRunningApp(h, context);
        }

        // If there's a game running, resume it
        if (h.getCurrentGame(serverInfo) != 0) {
            try {
                if (app.matchesRunningApp(details.runningGameId, details.runningGameUuid)) {
                    if (!launchApp(h, context, "resume")) {
                        if (context.streamConfig.isResumeOnly() && context.streamConfig.isIdleSessionEndAllowed() &&
                                hasEndedSession(h)) {
                            throw new SessionEndedException();
                        }
                        context.connListener.displayMessage(appContext.getString(R.string.startup_resume_failed));
                        return false;
                    }
                } else if (app.isSmallStreamInputOnly()) {
                    // Joining as a controller must never quit the game someone is playing.
                    context.connListener.displayMessage(appContext.getString(R.string.input_only_other_game));
                    return false;
                } else if (details.caps.has(HostCapabilities.Feature.HOST_CONFIRMS_REPLACEMENT)) {
                    // The host resolves control tiles and confirms replacement itself. Cancelling
                    // first would terminate the game when selecting its Resume or Remote Input tile.
                    return launchNotRunningApp(h, context);
                } else {
                    return quitAndLaunch(h, context);
                }
            } catch (HostHttpResponseException e) {
                // The app can exit between serverinfo and /resume. A 503 alone is not
                // evidence of an exit; encoder failures and a busy host remain errors.
                if (context.streamConfig.isResumeOnly() && context.streamConfig.isIdleSessionEndAllowed() &&
                        e.getErrorCode() == 503 && hasEndedSession(h)) {
                    throw new SessionEndedException();
                }
                if (e.getErrorCode() == 470) {
                    // This is the error you get when you try to resume a session that's not yours.
                    // Because this is fairly common, we'll display a more detailed message.
                    context.connListener.displayMessage(appContext.getString(R.string.startup_session_owner));
                    return false;
                }
                else if (e.getErrorCode() == 525) {
                    context.connListener.displayMessage(appContext.getString(R.string.startup_app_minimized));
                    return false;
                } else {
                    throw e;
                }
            }
            
            LimeLog.info("Resumed existing game session");
            return true;
        }
        else {
            return launchNotRunningApp(h, context);
        }
    }

    protected boolean quitAndLaunch(NvHTTP h, ConnectionContext context) throws IOException,
            XmlPullParserException {
        if (context.streamConfig.isResumeOnly()) {
            throw new LocalLaunchFailure(409, R.string.stream_recovery_session_changed,
                    "The previous session is no longer available.");
        }
        if (context.streamConfig.getApp().getControl() != NvApp.Control.NONE) {
            return launchNotRunningApp(h, context);
        }
        try {
            if (!h.quitApp()) {
                context.connListener.displayMessage(appContext.getString(R.string.startup_quit_failed));
                return false;
            } 
        } catch (HostHttpResponseException e) {
            if (e.getErrorCode() == 599) {
                context.connListener.displayMessage(appContext.getString(R.string.startup_session_owner));
                return false;
            }
            else {
                throw e;
            }
        }

        return launchNotRunningApp(h, context);
    }
    
    private boolean launchNotRunningApp(NvHTTP h, ConnectionContext context)
            throws IOException, XmlPullParserException {
        // Launch the app since it's not running
        if (!launchApp(h, context, "launch")) {
            if (!context.launchActionCompleted) {
                context.connListener.displayMessage(appContext.getString(R.string.startup_launch_failed));
            }
            return false;
        }
        
        LimeLog.info("Launched new game session");
        
        return true;
    }

    static boolean launchApp(NvHTTP http, ConnectionContext context, String verb)
            throws IOException, XmlPullParserException {
        if (context.streamConfig.isResumeOnly() && !"resume".equals(verb)) {
            throw new LocalLaunchFailure(409, R.string.stream_recovery_session_changed,
                    "Recovery can only resume the previous session.");
        }
        NvApp app = context.streamConfig.getApp();
        boolean retried = false;
        while (true) {
            try {
                return http.launchApp(context, verb, app.getAppId(), context.negotiatedHdr);
            } catch (HostHttpResponseException e) {
                if (LaunchConfirmation.isCompleted(app, e)) {
                    context.launchActionCompleted = true;
                    context.connListener.launchActionCompleted(e.getErrorMessage());
                    return false;
                }
                if (context.streamConfig.isResumeOnly() || !LaunchConfirmation.isRequired(app, e)) {
                    throw e;
                }
                if (retried) {
                    throw new LocalLaunchFailure(409, R.string.startup_confirmation_rejected,
                            "The host session changed or confirmation expired. Start the tile again.");
                }
                LaunchConfirmation confirmation = new LaunchConfirmation(
                        app.getControl() == NvApp.Control.TERMINATE, LaunchConfirmation.nowMs());
                try {
                    context.connListener.launchConfirmationRequired(confirmation);
                    LaunchConfirmation.State state = confirmation.await();
                    if (state == LaunchConfirmation.State.TIMED_OUT) {
                        throw new LocalLaunchFailure(408, R.string.startup_confirmation_timed_out,
                                "Confirmation timed out. Start the tile again to retry.");
                    } else if (state != LaunchConfirmation.State.CONFIRMED) {
                        throw new LocalLaunchFailure(499, R.string.startup_launch_cancelled, "Launch cancelled.");
                    }
                } catch (InterruptedException interrupted) {
                    confirmation.cancel();
                    Thread.currentThread().interrupt();
                    // A cancel from this device, never a transport failure that recovery may retry.
                    throw new LocalLaunchFailure(499, R.string.startup_launch_cancelled, "Launch cancelled.");
                } finally {
                    context.connListener.launchConfirmationFinished();
                }
                retried = true;
            }
        }
    }

    // A launch this device stopped itself: cancelled, timed out, or limited to resuming the
    // previous session. It keeps an HTTP-style code for the recovery policy, but the host
    // reported nothing, so it is never shown as a host failure.
    static final class LocalLaunchFailure extends HostHttpResponseException {
        private static final long serialVersionUID = 1L;
        final int messageId;

        LocalLaunchFailure(int errorCode, int messageId, String logMessage) {
            super(errorCode, logMessage);
            this.messageId = messageId;
        }
    }

    // The host's own reason is shown when it gave one (Sunshine explains encoder and capture
    // failures this way); the generic text only stands in when it gave none.
    static String launchFailureMessage(Context context, HostHttpResponseException e) {
        if (e instanceof LocalLaunchFailure) {
            return context.getString(((LocalLaunchFailure) e).messageId);
        }
        String reason = e.getErrorMessage() == null ? "" : e.getErrorMessage().trim();
        if (e.getErrorCode() == 401 || e.getErrorCode() == 403) {
            String permission = context.getString(R.string.startup_permission_denied);
            return reason.isEmpty() ? permission : permission + "\n\n" + reason;
        }
        return reason.isEmpty() ? context.getString(R.string.startup_host_error, e.getErrorCode()) :
                context.getString(R.string.startup_host_message, reason, e.getErrorCode());
    }

    public synchronized void start(final AudioRenderer audioRenderer, final VideoDecoderRenderer videoDecoderRenderer, final NvConnectionListener connectionListener)
    {
        if (connectionThread != null || stopRequested) {
            return;
        }
        connectionThread = new Thread(new Runnable() {
            public void run() {
                context.connListener = connectionListener;
                String appName = context.streamConfig.getApp().getAppName();
                boolean acquired = false;
                boolean started = false;
                try {
                    if (stopRequested) {
                        return;
                    }
                    context.videoCapabilities = videoDecoderRenderer.getCapabilities();
                    context.connListener.stageStarting(appName);
                    connectionAllowed.acquire();
                    acquired = true;
                    if (!startApp(videoDecoderRenderer)) {
                        if (!stopRequested && !context.launchActionCompleted) {
                            context.connListener.stageFailed(appName, 0, 0);
                        }
                        return;
                    }
                    if (stopRequested) {
                        return;
                    }
                    context.connListener.stageComplete(appName);

                    ByteBuffer ib = ByteBuffer.allocate(16);
                    ib.putInt(context.riKeyId);

                    // Pacing hints for the host, which sends frames in bursts at its own link
                    // rate: this device's wired port speed toward it, for every codec so a host
                    // can cap its pacing, and for PyroWave the frame pace calibrated on this
                    // same path (PyroWavePaceCalibration), if any. Zero (Wi-Fi, cellular,
                    // unknown) leaves either out of the stream description.
                    boolean pyroWave = (context.negotiatedVideoFormats & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0;
                    int pyroWaveLinkMbps = WiredLink.mbpsToward(appContext, context.serverAddress.address);
                    int pyroWavePaceMbps = pyroWave ?
                            PyroWavePaceStore.paceMbpsFor(appContext, hostDetails.uuid, pyroWaveLinkMbps) : 0;
                    if (pyroWave) {
                        LimeLog.info("PyroWave pacing hints: link " + pyroWaveLinkMbps + " Mbps, calibrated pace " +
                                pyroWavePaceMbps + " Mbps");
                    } else if (pyroWaveLinkMbps > 0) {
                        LimeLog.info("Pacing hint: link " + pyroWaveLinkMbps + " Mbps");
                    }

                    synchronized (MoonBridge.class) {
                        synchronized (NvConnection.this) {
                            if (stopRequested) {
                                return;
                            }
                            nativeStarted = true;
                        }
                        MoonBridge.setupBridge(videoDecoderRenderer, audioRenderer, connectionListener, () -> stopRequested);
                        PreferenceConfiguration prefs = PreferenceConfiguration.readPreferences(appContext);
                        micEnabled = prefs.enableMic;
                        micPermitted = prefs.enableMic && hasMicrophonePermission();
                        if (micEnabled && !micPermitted) {
                            LimeLog.warning("Microphone: on in settings but the permission is not granted; not sending");
                        }
                        int graceMs = NetworkGrace.nativeMilliseconds(prefs.networkGraceSeconds);
                        if (graceMs > 0) {
                            LimeLog.info("Network drop tolerance: " + prefs.networkGraceSeconds + " s");
                        }
                        MoonBridge.setNetworkGracePeriodMs(graceMs);
                        MoonBridge.requestRustEngine(prefs.rustEngine);
                        MoonBridge.requestPacked444(prefs.experimentalPacked444);
                        int ret = MoonBridge.startConnection(context.serverAddress.address,
                                context.serverAppVersion, context.serverGfeVersion, context.rtspSessionUrl,
                                context.serverCodecModeSupport,
                                context.negotiatedWidth, context.negotiatedHeight,
                                context.streamConfig.getRefreshRate(), context.streamConfig.getBitrate(),
                                context.negotiatedPacketSize, context.negotiatedRemoteStreaming,
                                context.streamConfig.getAudioConfiguration().toInt(),
                                context.negotiatedVideoFormats,
                                context.streamConfig.getClientRefreshRateX100(),
                                context.riKey.getEncoded(), ib.array(),
                                context.videoCapabilities,
                                context.streamConfig.getColorSpace(),
                                context.streamConfig.getColorRange(), prefs.unbatchedInput, prefs.networkPriority,
                                hostDetails.caps.nativeFeatures(),
                                context.streamConfig.getApp().getRole() == NvApp.Role.INPUT_ONLY &&
                                        !context.streamConfig.getApp().isSmallStreamInputOnly(),
                                context.streamConfig.getApp().getRole() == NvApp.Role.REMOTE_MONITOR,
                                micPermitted, prefs.usbDualSenseHaptics,
                                pyroWaveLinkMbps, pyroWavePaceMbps);
                        if (ret != 0) {
                            return;
                        }
                        nativeConnected = true;
                        startInputPolling(prefs.unbatchedInput);
                        startMicrophone();
                        started = true;
                    }
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                } catch (SessionEndedException e) {
                    if (!stopRequested) {
                        context.connListener.connectionTerminated(MoonBridge.ML_ERROR_GRACEFUL_TERMINATION);
                    }
                } catch (HostHttpResponseException e) {
                    if (!stopRequested) {
                        e.printStackTrace();
                        context.connListener.displayMessage(launchFailureMessage(appContext, e));
                        context.connListener.stageFailed(appName, 0, e.getErrorCode());
                    }
                } catch (XmlPullParserException | IOException | RuntimeException e) {
                    if (!stopRequested) {
                        e.printStackTrace();
                        context.connListener.displayMessage(appContext.getString(R.string.startup_connection_failed));
                        // Only transport failures qualify for automatic recovery. Malformed host
                        // data, TLS/certificate rejection and client errors need a user decision.
                        int retryPorts = e instanceof IOException && !(e instanceof javax.net.ssl.SSLException) ?
                                MoonBridge.ML_PORT_FLAG_TCP_47984 | MoonBridge.ML_PORT_FLAG_TCP_47989 : 0;
                        context.connListener.stageFailed(appName, retryPorts, 0);
                    }
                } finally {
                    if (!started) {
                        try {
                            synchronized (MoonBridge.class) {
                                synchronized (NvConnection.this) {
                                    if (nativeStarted) {
                                        nativeStarted = false;
                                        MoonBridge.cleanupBridge();
                                    }
                                }
                                videoDecoderRenderer.cleanup();
                            }
                        } finally {
                            if (acquired) {
                                connectionAllowed.release();
                            }
                        }
                    }
                }
            }
        });
        connectionThread.start();
    }
    
    public ComputerDetails getHostDetails() {
        return hostDetails;
    }

    public synchronized boolean startHardwareMetricsPolling(Consumer<HardwareMetrics> listener) {
        if (stopRequested || hardwareMetricsUnavailable || !hostDetails.supportsHardwareMetrics() ||
                context.streamConfig.getApp().getRole() == NvApp.Role.INPUT_ONLY) return false;
        if (hardwareMetricsPoller != null) return true;
        ScheduledExecutorService poller = Executors.newSingleThreadScheduledExecutor(r -> new Thread(r, "Host metrics"));
        hardwareMetricsPoller = poller;
        poller.execute(() -> pollHardwareMetrics(poller, listener));
        return true;
    }

    private void pollHardwareMetrics(ScheduledExecutorService poller, Consumer<HardwareMetrics> listener) {
        HardwareMetrics metrics = null;
        long delayMs = 10000;
        boolean unavailable = false;
        try {
            NvHTTP request;
            synchronized (this) {
                if (hardwareMetricsPoller != poller) return;
                request = hardwareMetricsHttp;
            }
            if (request == null) {
                request = new NvHTTP(context.serverAddress, hostDetails.httpsPort, uniqueId, context.serverCert, cryptoProvider);
                synchronized (this) {
                    if (hardwareMetricsPoller != poller) {
                        request.cancelPendingRequests();
                        return;
                    }
                    hardwareMetricsHttp = request;
                }
            }
            metrics = request.getHardwareMetrics();
            delayMs = Math.max(2000, metrics.intervalMs);
        } catch (FileNotFoundException | HardwareMetrics.UnsupportedVersionException e) {
            unavailable = true;
        } catch (HostHttpResponseException e) {
            unavailable = e.getErrorCode() == 403 || e.getErrorCode() == 404 || e.getErrorCode() == 409;
        } catch (IOException e) {
            // A failed optional sample must not interrupt the stream or preserve old values.
        }
        synchronized (this) {
            if (hardwareMetricsPoller != poller) return;
            if (unavailable) {
                hardwareMetricsUnavailable = true;
                stopHardwareMetricsPolling();
            }
        }
        listener.accept(metrics);
        synchronized (this) {
            if (hardwareMetricsPoller == poller) {
                poller.schedule(() -> pollHardwareMetrics(poller, listener), delayMs, TimeUnit.MILLISECONDS);
            }
        }
    }

    public synchronized void stopHardwareMetricsPolling() {
        if (hardwareMetricsHttp != null) {
            hardwareMetricsHttp.cancelPendingRequests();
            hardwareMetricsHttp = null;
        }
        if (hardwareMetricsPoller != null) {
            hardwareMetricsPoller.shutdownNow();
            hardwareMetricsPoller = null;
        }
    }

    public boolean canSendInput() {
        return !stopRequested && !isMonkey && context.streamConfig.getApp().getRole() != NvApp.Role.REMOTE_MONITOR &&
                hostDetails.hasPermission(ComputerDetails.PERMISSION_INPUT);
    }

    public boolean canSendControllerInput() {
        return canSendInput() && hostDetails.hasPermission(ComputerDetails.PERMISSION_CONTROLLER);
    }

    public ComputerDetails refreshHostDetails() throws IOException, XmlPullParserException {
        NvHTTP connection;
        synchronized (this) {
            if (stopRequested || http == null) {
                throw new IOException("Stream is disconnected");
            }
            connection = http;
        }
        ComputerDetails details = connection.getComputerDetails(true);
        if (details.pairState != PairingManager.PairState.PAIRED) {
            throw new IOException("Device is no longer paired with the host");
        }
        hostDetails = details;
        return details;
    }

    /** Confirm an intentional host exit without mistaking an unreachable host for an idle one. */
    public boolean hasEndedSession() {
        NvHTTP connection;
        synchronized (this) {
            if (stopRequested || http == null) return false;
            connection = http;
        }
        return hasEndedSession(connection);
    }

    private boolean hasEndedSession(NvHTTP connection) {
        NvApp app = context.streamConfig.getApp();
        if (stopRequested || app.getRole() != NvApp.Role.STREAM || app.isControlAction()) return false;
        try {
            // Pinned HTTPS, no HTTP fallback, and a three-second deadline. stop() also
            // cancels this request so leaving the activity cannot strand teardown.
            ComputerDetails details = connection.probePairedComputer();
            return !stopRequested && details.pairState == PairingManager.PairState.PAIRED &&
                    details.hasPermission(ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_LAUNCH) &&
                    details.sessionIdle && !details.hasRunningGame();
        } catch (IOException | XmlPullParserException e) {
            return false;
        }
    }

    public String getClipboard() throws IOException, XmlPullParserException {
        if (!refreshHostDetails().canReadClipboard()) {
            throw new IOException("Host clipboard read permission denied");
        }
        return http.getClipboard();
    }

    public void quitApp() throws IOException, XmlPullParserException {
        if (isMonkey || context.streamConfig.getApp().getRole() != NvApp.Role.STREAM ||
                !refreshHostDetails().hasPermission(ComputerDetails.PERMISSION_LAUNCH)) {
            throw new IOException("The host denied permission to quit this app.");
        }
        if (!http.quitApp()) {
            throw new IOException("The host did not quit the app.");
        }
    }

    /**
     * Runs a native API session action on the host: Disconnect, End session or Close game. Only
     * for hosts that have the native API; the others quit with {@link #quitApp()}.
     */
    public NativeSession nativeSessionAction(NativeSession.ActionId action) throws IOException, XmlPullParserException {
        NativeApiClient client = nativeClientForAction();
        try {
            return client.sessionAction(action);
        } catch (NativeApiException e) {
            throw new IOException(e.getMessage(), e);
        }
    }

    /** The native client once the host, this device's launch permission and the stream role allow a session action. */
    private NativeApiClient nativeClientForAction() throws IOException, XmlPullParserException {
        ComputerDetails details = refreshHostDetails();
        if (isMonkey || context.streamConfig.getApp().getRole() != NvApp.Role.STREAM ||
                !details.hasPermission(ComputerDetails.PERMISSION_LAUNCH)) {
            throw new IOException("The host denied permission to change this session.");
        }
        if (!details.caps.has(HostCapabilities.Feature.NATIVE_API)) {
            throw new IOException("The host has no native session actions.");
        }
        NativeApiClient client = nativeClient();
        if (client == null) {
            throw new IOException("The host's certificate is not known.");
        }
        return client;
    }

    /** Whether the host has the native API session actions, as its last serverinfo said. */
    public boolean hasNativeSessionActions() {
        return hostDetails.caps.has(HostCapabilities.Feature.NATIVE_API);
    }

    /**
     * Closes the game on the host. A native API host closes it through the native API when this
     * device has host control; without that permission, and on every other host, it quits the app
     * through /cancel, which needs no such permission.
     */
    public void closeGame() throws IOException, XmlPullParserException {
        if (hasNativeSessionActions()) {
            try {
                if (nativeClientForAction().closeGameIfAllowed()) {
                    return;
                }
            } catch (NativeApiException e) {
                throw new IOException(e.getMessage(), e);
            }
        }
        quitApp();
    }

    /** Whether this device may close the game through the native API, as the host last said; false until known. */
    public boolean hasNativeHostControl() {
        return nativeHostControl;
    }

    /** Asks a native API host whether this device has host control, in the background; the stream menu reads the answer. */
    public void refreshNativeHostControl() {
        if (isMonkey || !hasNativeSessionActions()) {
            return;
        }
        NativeApiClient client = nativeClient();
        if (client == null) {
            return;
        }
        new Thread(() -> {
            try {
                nativeHostControl = client.capabilities().hostControl;
            } catch (NativeApiException e) {
                LimeLog.info("Native capabilities not read: " + e.getMessage());
            }
        }, "Native capabilities").start();
    }

    /**
     * A native API client for the stream menu's Radeon panel, or null when this host or stream has no
     * native API. Its calls block: run them off the render, decode and input threads.
     */
    public NativeApiClient nativeApiClient() {
        if (isMonkey || context.streamConfig.getApp().getRole() != NvApp.Role.STREAM || !hasNativeSessionActions()) {
            return null;
        }
        return nativeClient();
    }

    private NativeApiClient nativeClient() {
        HostEndpoint endpoint = HostEndpoint.of(hostDetails.uuid, context.serverAddress, context.httpsPort,
                context.serverCert, uniqueId, cryptoProvider);
        return endpoint == null ? null : new NativeApiClient(nativeApiTransport, endpoint);
    }

    /**
     * Tells a native API host that this device leaves on purpose, so the game keeps running
     * under the host's grace period. Never waits and ignores failures: the stream ending says
     * the same to a host that does not get it.
     */
    public void announceDisconnect() {
        if (isMonkey || context.streamConfig.getApp().getRole() != NvApp.Role.STREAM || !hasNativeSessionActions()) {
            return;
        }
        HostEndpoint endpoint = HostEndpoint.of(hostDetails.uuid, context.serverAddress, context.httpsPort,
                context.serverCert, uniqueId, cryptoProvider);
        if (endpoint == null) {
            return;
        }
        NativeApiClient client = new NativeApiClient(nativeApiTransport, endpoint);
        new Thread(() -> {
            try {
                client.sessionAction(NativeSession.ActionId.DISCONNECT);
            } catch (NativeApiException e) {
                LimeLog.info("Native disconnect not delivered: " + e.getMessage());
            }
        }, "Native disconnect").start();
    }

    /** Replaces the native API transport; the JNI one by default. */
    public void setNativeApiTransport(NativeApiTransport transport) {
        nativeApiTransport = transport;
    }

    public void disconnectRemoteSession() throws IOException, XmlPullParserException {
        ComputerDetails details = refreshHostDetails();
        if (!details.hasPermission(ComputerDetails.PERMISSION_LAUNCH)) {
            throw new IOException("The host denied permission to end this remote session.");
        }
        if (!details.caps.mayHave(HostCapabilities.Feature.REMOTE_SESSION_TILES)) {
            // Apollo has no disconnect tile: its Remote Input session ends when this stream does.
            return;
        }
        if (!http.disconnectRole(context.streamConfig.getApp().getRole())) {
            throw new IOException("The host did not end the remote session.");
        }
    }

    public int setBitrate(int kbps) throws IOException, XmlPullParserException {
        ComputerDetails details = refreshHostDetails();
        if (isMonkey || context.streamConfig.getApp().getRole() == NvApp.Role.INPUT_ONLY ||
                !details.caps.has(HostCapabilities.Feature.RUNTIME_BITRATE) ||
                !details.hasPermission(ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_LAUNCH)) {
            throw new IOException("Host does not allow runtime bitrate changes");
        }
        return http.setBitrate(kbps);
    }

    public void sendClipboard(String text) throws IOException, XmlPullParserException {
        if (context.streamConfig.getApp().getRole() == NvApp.Role.REMOTE_MONITOR || !refreshHostDetails().canWriteClipboard()) {
            throw new IOException("Host clipboard write permission denied");
        }
        if (!isMonkey) {
            http.sendClipboard(text);
        }
    }

    public boolean sendServerCommand(int index, String name) throws IOException, XmlPullParserException {
        ComputerDetails details = refreshHostDetails();
        if (context.streamConfig.getApp().getRole() == NvApp.Role.REMOTE_MONITOR ||
                !details.canRunServerCommand(index) || !details.serverCommands.get(index).equals(name)) {
            throw new IOException("Server command changed or permission denied. Reopen the stream menu.");
        }
        synchronized (MoonBridge.class) {
            if (stopRequested || !nativeConnected || isMonkey) {
                return false;
            }
            long now = System.nanoTime();
            if (lastServerCommandTime != 0 && now - lastServerCommandTime < 1_000_000_000L) {
                throw new IOException("Wait one second between server commands");
            }
            lastServerCommandTime = now;
            return MoonBridge.sendServerCommand(details.serverCommandPayload(index));
        }
    }

    public void sendMouseMove(final short deltaX, final short deltaY)
    {
        if (canSendInput()) {
            if (inputPoller != null) inputBatcher.mouse(0, deltaX, deltaY, (short) 0, (short) 0);
            else MoonBridge.sendMouseMove(deltaX, deltaY);
        }
    }

    public void sendMousePosition(short x, short y, short referenceWidth, short referenceHeight)
    {
        if (canSendInput()) {
            if (inputPoller != null) inputBatcher.mouse(1, x, y, referenceWidth, referenceHeight);
            else MoonBridge.sendMousePosition(x, y, referenceWidth, referenceHeight);
        }
    }

    public void sendMouseMoveAsMousePosition(short deltaX, short deltaY, short referenceWidth, short referenceHeight)
    {
        if (canSendInput()) {
            if (inputPoller != null) inputBatcher.mouse(2, deltaX, deltaY, referenceWidth, referenceHeight);
            else MoonBridge.sendMouseMoveAsMousePosition(deltaX, deltaY, referenceWidth, referenceHeight);
        }
    }

    public void sendMouseButtonDown(final byte mouseButton)
    {
        sendOwnedMouseButton(mouseButton, true, com.limelight.binding.input.MouseButtonOwnership.ORDINARY);
    }
    
    public void sendMouseButtonUp(final byte mouseButton)
    {
        sendOwnedMouseButton(mouseButton, false, com.limelight.binding.input.MouseButtonOwnership.ORDINARY);
    }

    private final com.limelight.binding.input.MouseButtonOwnership mouseButtonOwnership =
            new com.limelight.binding.input.MouseButtonOwnership();

    public void sendOverlayMouseButton(byte mouseButton, boolean down) {
        sendOwnedMouseButton(mouseButton, down, com.limelight.binding.input.MouseButtonOwnership.GAME_CONTROLS);
    }

    public void sendDesktopMouseButton(byte mouseButton, boolean down) {
        sendOwnedMouseButton(mouseButton, down, com.limelight.binding.input.MouseButtonOwnership.DESKTOP_CONTROLS);
    }

    private void sendOwnedMouseButton(byte mouseButton, boolean down, int source) {
        // Losing permission must not leave this surface owning a button locally. Clear its
        // release even when nothing may be sent, so a later permitted press can start again.
        if (!down || canSendInput()) {
            inputBatcher.boundary(() -> {
                synchronized (mouseButtonOwnership) {
                    // Permission may have changed while waiting for the input batch lock.
                    if (down && !canSendInput()) return;
                    int transition = mouseButtonOwnership.update(mouseButton, down, source);
                    if (canSendInput() && transition != 0) MoonBridge.sendMouseButton(
                            transition > 0 ? MouseButtonPacket.PRESS_EVENT : MouseButtonPacket.RELEASE_EVENT, mouseButton);
                }
            });
        }
    }
    
    public void sendControllerInput(final short controllerNumber,
            final short activeGamepadMask, final int buttonFlags,
            final byte leftTrigger, final byte rightTrigger,
            final short leftStickX, final short leftStickY,
            final short rightStickX, final short rightStickY)
    {
        if (canSendControllerInput()) {
            Runnable send = () -> {
                if (canSendControllerInput()) MoonBridge.sendMultiControllerInput(controllerNumber, activeGamepadMask, buttonFlags,
                        leftTrigger, rightTrigger, leftStickX, leftStickY, rightStickX, rightStickY);
            };
            if (inputPoller != null) inputBatcher.controller(controllerNumber, activeGamepadMask, buttonFlags,
                    leftTrigger, rightTrigger, send);
            else send.run();
        }
    }

    public void sendKeyboardInput(final short keyMap, final byte keyDirection, final byte modifier, final byte flags) {
        if (canSendInput()) {
            inputBatcher.boundary(() -> {
                if (canSendInput()) MoonBridge.sendKeyboardInput(keyMap, keyDirection, modifier, flags);
            });
        }
    }
    
    public void sendMouseScroll(final byte scrollClicks) {
        if (canSendInput()) {
            inputBatcher.boundary(() -> {
                if (canSendInput()) MoonBridge.sendMouseHighResScroll((short)(scrollClicks * 120)); // WHEEL_DELTA
            });
        }
    }

    public void sendMouseHScroll(final byte scrollClicks) {
        if (canSendInput()) {
            inputBatcher.boundary(() -> {
                if (canSendInput()) MoonBridge.sendMouseHighResHScroll((short)(scrollClicks * 120)); // WHEEL_DELTA
            });
        }
    }

    public void sendMouseHighResScroll(final short scrollAmount) {
        if (canSendInput()) {
            inputBatcher.boundary(() -> {
                if (canSendInput()) MoonBridge.sendMouseHighResScroll(scrollAmount);
            });
        }
    }

    public void sendMouseHighResHScroll(final short scrollAmount) {
        if (canSendInput()) {
            inputBatcher.boundary(() -> {
                if (canSendInput()) MoonBridge.sendMouseHighResHScroll(scrollAmount);
            });
        }
    }

    public int sendTouchEvent(byte eventType, int pointerId, float x, float y, float pressureOrDistance,
                              float contactAreaMajor, float contactAreaMinor, short rotation) {
        if (canSendInput()) {
            return MoonBridge.sendTouchEvent(eventType, pointerId, x, y, pressureOrDistance,
                    contactAreaMajor, contactAreaMinor, rotation);
        }
        else {
            return MoonBridge.LI_ERR_UNSUPPORTED;
        }
    }

    public int sendPenEvent(byte eventType, byte toolType, byte penButtons, float x, float y,
                            float pressureOrDistance, float contactAreaMajor, float contactAreaMinor,
                            short rotation, byte tilt) {
        if (canSendInput()) {
            return MoonBridge.sendPenEvent(eventType, toolType, penButtons, x, y, pressureOrDistance,
                    contactAreaMajor, contactAreaMinor, rotation, tilt);
        }
        else {
            return MoonBridge.LI_ERR_UNSUPPORTED;
        }
    }

    public int sendControllerArrivalEvent(byte controllerNumber, short activeGamepadMask, byte type,
                                          int supportedButtonFlags, short capabilities) {
        return canSendControllerInput() ? MoonBridge.sendControllerArrivalEvent(controllerNumber, activeGamepadMask,
                type, supportedButtonFlags, capabilities) : MoonBridge.LI_ERR_UNSUPPORTED;
    }

    public int sendControllerTouchEvent(byte controllerNumber, byte eventType, int pointerId,
                                        float x, float y, float pressure) {
        if (canSendControllerInput()) {
            return MoonBridge.sendControllerTouchEvent(controllerNumber, eventType, pointerId, x, y, pressure);
        }
        else {
            return MoonBridge.LI_ERR_UNSUPPORTED;
        }
    }

    public int sendControllerMotionEvent(byte controllerNumber, byte motionType,
                                         float x, float y, float z) {
        if (canSendControllerInput()) {
            return MoonBridge.sendControllerMotionEvent(controllerNumber, motionType, x, y, z);
        }
        else {
            return MoonBridge.LI_ERR_UNSUPPORTED;
        }
    }

    public void sendControllerBatteryEvent(byte controllerNumber, byte batteryState, byte batteryPercentage) {
        if (canSendControllerInput()) {
            MoonBridge.sendControllerBatteryEvent(controllerNumber, batteryState, batteryPercentage);
        }
    }

    public void sendUtf8Text(final String text) {
        if (canSendInput()) {
            MoonBridge.sendUtf8Text(text.getBytes(StandardCharsets.UTF_8));
        }
    }

    public static String findExternalAddressForMdns(String stunHostname, int stunPort) {
        return MoonBridge.findExternalAddressIP4(stunHostname, stunPort);
    }
}
