package com.limelight;


import com.limelight.binding.PlatformBinding;
import com.limelight.binding.audio.AndroidAudioRenderer;
import com.limelight.binding.audio.SilentAudioRenderer;
import com.limelight.binding.input.ControllerHandler;
import com.limelight.binding.input.KeyboardTranslator;
import com.limelight.binding.input.capture.InputCaptureManager;
import com.limelight.binding.input.capture.InputCaptureProvider;
import com.limelight.binding.input.touch.TouchMode;
import com.limelight.binding.input.touch.TrackpadPointer;
import com.limelight.binding.input.driver.UsbDriverService;
import com.limelight.binding.input.evdev.EvdevListener;
import com.limelight.binding.input.virtual_controller.VirtualController;
import com.limelight.binding.video.CrashAccounting;
import com.limelight.binding.video.CrashListener;
import com.limelight.binding.video.DecoderSetupFailure;
import com.limelight.binding.video.PackedFallback;
import com.limelight.binding.video.DiscardingDecoderRenderer;
import com.limelight.binding.video.DisplayCapsReporter;
import com.limelight.binding.video.DisplayRefreshDiagnostics;
import com.limelight.binding.video.HdrFormatPolicy;
import com.limelight.binding.video.HdrPolicy;
import com.limelight.binding.video.LatencyProbe;
import com.limelight.binding.video.LinkNoticeStatus;
import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.binding.video.PerfOverlayListener;
import com.limelight.binding.video.PerformanceOverlay;
import com.limelight.binding.video.Sdr10Policy;
import com.limelight.binding.video.StagedStreamExit;
import com.limelight.binding.video.StreamResizePolicy;
import com.limelight.binding.video.UpscalingPolicy;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.computers.HostNotices;
import com.limelight.nativeapi.NativeSession;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.AdaptiveBitrateController;
import com.limelight.nvstream.AdaptiveQualityController;
import com.limelight.nvstream.PyroWaveBitrateController;
import com.limelight.nvstream.RateChangeResult;
import com.limelight.nvstream.NvConnectionListener;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ClipboardImageException;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.LaunchConfirmation;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.AppQuitOnLeave;
import com.limelight.preferences.AutoBitrateChoice;
import com.limelight.preferences.GlPreferences;
import com.limelight.preferences.HostDisplayChoice;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.BitrateSlider;
import com.limelight.ui.GameGestures;
import com.limelight.ui.HostCursorShapes;
import com.limelight.ui.KeyboardOverlay;
import com.limelight.ui.LegacyKeyboardInsets;
import com.limelight.ui.PcKeyboardOverlay;
import com.limelight.ui.StreamOrientation;
import com.limelight.ui.StreamView;
import com.limelight.ui.VideoScaling;
import com.limelight.utils.ClipboardImages;
import com.limelight.utils.Dialog;
import com.limelight.utils.HdrBrightness;
import com.limelight.utils.NetworkGrace;
import com.limelight.utils.LaunchSource;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.ShortcutHelper;
import com.limelight.utils.SpinnerDialog;
import com.limelight.utils.UiHelper;

import android.annotation.SuppressLint;
import android.annotation.TargetApi;
import android.app.Activity;
import androidx.appcompat.app.AlertDialog;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.core.view.WindowInsetsControllerCompat;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.ui.NavigationDialogBuilder;
import android.app.PictureInPictureParams;
import android.app.PendingIntent;
import android.app.RemoteAction;
import android.content.BroadcastReceiver;
import android.content.IntentFilter;
import android.graphics.drawable.Icon;
import com.limelight.ui.PictureInPicturePolicy;
import android.app.Service;
import android.content.ComponentName;
import android.content.Context;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.content.res.Configuration;
import android.graphics.Point;
import android.graphics.SurfaceTexture;
import android.graphics.Rect;
import android.hardware.input.InputManager;
import android.media.AudioManager;
import android.media.MediaFormat;
import android.net.ConnectivityManager;
import android.net.wifi.WifiManager;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.os.IBinder;
import android.os.PersistableBundle;
import android.preference.PreferenceManager;
import android.text.InputType;
import android.util.Rational;
import android.view.Display;
import android.view.InputDevice;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.Surface;
import android.view.SurfaceHolder;
import android.view.TextureView;
import android.view.View;
import android.view.View.OnGenericMotionListener;
import android.view.View.OnSystemUiVisibilityChangeListener;
import android.view.View.OnTouchListener;
import android.view.Window;
import android.view.WindowManager;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.TextView;
import android.widget.Toast;

import org.xmlpull.v1.XmlPullParserException;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import com.limelight.ui.ActionSheet;
import java.math.BigDecimal;
import java.util.Locale;


public class Game extends Activity implements SurfaceHolder.Callback, TextureView.SurfaceTextureListener,
        OnGenericMotionListener, OnTouchListener, NvConnectionListener, EvdevListener,
        OnSystemUiVisibilityChangeListener, GameGestures, StreamView.InputCallbacks,
        PerfOverlayListener, UsbDriverService.UsbDriverStateListener, View.OnKeyListener,
        com.limelight.ui.StreamWindow {
    // Each concern of the stream lives in a helper that holds this activity.
    final GameOverlays overlays = new GameOverlays(this);
    final GameKeyboardInput keyboardInput = new GameKeyboardInput(this);
    final GameTouchInput touchInput = new GameTouchInput(this);

    TrackpadPointer trackpadPointer;
    // Set while the host sends its cursor shapes for the Local cursor; read on the control thread.
    volatile HostCursorShapes hostCursorShapes;
    boolean nativeTouchEnabled;

    ControllerHandler controllerHandler;
    KeyboardTranslator keyboardTranslator;
    KeyboardOverlay keyboardOverlay;
    PcKeyboardOverlay pcKeyboard;
    private final InputManager.InputDeviceListener pcKeyboardDeviceListener = new InputManager.InputDeviceListener() {
        // A keyboard plugged in or switched to another layout changes what the host should type.
        @Override public void onInputDeviceAdded(int deviceId) { keyboardInput.reportKeyboardLayout(); }
        @Override public void onInputDeviceChanged(int deviceId) { keyboardInput.reportKeyboardLayout(); }
        @Override public void onInputDeviceRemoved(int deviceId) { runOnUiThread(keyboardInput::releasePcKeyboardPads); }
    };
    boolean desktopWindowFocused = true;
    boolean dispatchingDesktopTouch;
    VirtualController virtualController;
    // The modern on-screen controller, used instead of the classic one when the settings ask for it.
    com.limelight.binding.input.pad.ModernController modernController;
    com.limelight.ui.MicButton micButton;

    PreferenceConfiguration prefConfig;
    private SharedPreferences tombstonePrefs;

    NvConnection conn;
    private SpinnerDialog spinner;
    private boolean displayedFailureDialog = false;
    private boolean connecting = false;
    volatile boolean connected = false;
    private boolean autoEnterPip = false;
    private com.limelight.ui.AdaptiveLayout adaptiveLayout;
    private boolean surfaceCreated = false;
    private boolean attemptedConnection = false;
    private volatile Thread connectionStopThread;
    private StreamRecoveryPolicy recoveryPolicy = new StreamRecoveryPolicy(0, 0);
    private long connectedAt = -1;
    // How long the stream had been connected when the user's trip began, carried by its resume.
    private long connectedBeforeTrip;
    int recoveryAppId;
    String recoveryAppUuid;
    private Runnable pendingRecovery;
    // Volatile: a recovery's route wait on its worker stops when this changes.
    private volatile int recoveryGeneration;
    private boolean recoveryHandoffInProgress;
    // The replacement stream a queued recovery will start; a packed-chroma failure meanwhile marks it.
    private Intent pendingRecoveryIntent;
    // Set when the last automatic recovery was refused because its attempts or window ran out.
    private boolean recoveryBudgetExhausted;
    private boolean recoveryCancelledOnPause;
    private Intent stoppedStreamResumeIntent;
    private final Runnable recoveryTimeout = () -> {
        if (this.connecting && this.foreground && !isFinishing() && isRecoveryAttempt()) {
            displayedFailureDialog = true;
            stopConnection();
            if (spinner != null) { spinner.dismiss(); spinner = null; }
            showReconnectDialog(R.string.conn_terminated_title, getString(R.string.stream_recovery_exhausted));
        }
    };
    private int suppressPipRefCount = 0;
    android.app.Dialog streamMenu;
    // The sectioned stream menu while it is open; streamMenu is its sheet or a dialog it opened.
    private com.limelight.ui.StreamMenu streamMenuSheet;
    private Runnable streamMenuAction;
    private androidx.appcompat.app.AlertDialog launchConfirmationDialog;
    private LaunchConfirmation pendingConfirmation;
    private String connectionErrorMessage;
    private boolean hostActionInProgress;
    // Set while this device asks the host to end the session. Hosts drop the stream while
    // handling that request, often without an end message, so any termination is expected.
    private volatile boolean hostExitRequested;
    // A failed request may still have ended the session: the host can drop the stream before the
    // request fails, and the lost transport then takes up to its 10 s timeout to report. The
    // request has only failed once the stream is still connected after that.
    private static final long HOST_EXIT_CONFIRM_MS = 15000;
    private final Runnable hostExitFailed = () -> {
        if (connected && !hostActionInProgress) hostExitRequested = false;
    };
    boolean foreground;
    private int foregroundGeneration;
    private boolean restoreInputAfterMenu;
    private boolean restoreInputOnResume;
    private final WindowFocusActionQueue windowFocusActions = new WindowFocusActionQueue();
    private final Runnable runWindowFocusActions = () -> {
        if (!foreground || !connected || isFinishing()) {
            windowFocusActions.clear();
        } else if (streamMenu == null) {
            windowFocusActions.runPending(hasWindowFocus(), SystemClock.uptimeMillis());
        }
    };
    int currentBitrate;
    private AdaptiveBitrateController adaptiveBitrate;
    private PyroWaveBitrateController pyroWaveBitrate;
    private boolean poorConnection;
    // The stream's network is lost: the overlay says so until one returns (or the stream ends).
    private boolean streamNetworkLost;
    private long adaptiveBitrateRetryAt;
    private int adaptiveBitrateRetryTarget;
    // Claims of the host's encoder bitrate that failed in a row. Some hosts report 0 for good.
    private static final int MAX_BITRATE_CLAIM_FAILURES = 6;
    private int adaptiveBitrateClaimFailures;
    // A Foundation Sunshine PC's own bitrate controller, on in place of this device's.
    private com.limelight.nvstream.http.FoundationAbr foundationAbr;
    // Automatic quality switched auto bitrate on for itself; the person's own choice was off.
    private boolean qualityAutoBitrate;
    AdaptiveQualityController adaptiveQuality;
    private AdaptiveQualityController.Target pendingQualityTarget;
    private long qualityRequestSentAt;
    String qualityStatus;
    private int qualityMaxWidth, qualityMaxHeight, qualityMaxFps;
    private int qualityMaxFrameRateMilliHz;
    final Handler bitrateHandler = new Handler(Looper.getMainLooper());
    final com.limelight.utils.StreamHealth streamHealth = new com.limelight.utils.StreamHealth();
    private com.limelight.utils.WifiDiagnostics wifiDiagnostics;
    private int healthOriginalBitrate, healthTrialBitrate;
    // 0x553C rate changes, for a Rust stream whose host offered them; null otherwise.
    private StreamRateChanges rateChanges;
    // Automatic bitrate on this device: one client-hint sample a second, as a 0x553C request.
    private final Runnable updateRateChangeBitrate = new Runnable() {
        @Override public void run() {
            if (!connected || rateChanges == null || !rateChanges.automaticOn() || decoderRenderer == null) return;
            long now = SystemClock.uptimeMillis();
            if (!hostActionInProgress && streamMenu == null && !rateChanges.sampleAutomatic(now,
                    decoderRenderer.hasRecentVideoFrames(now), decoderRenderer.getNetworkFrameLossPercent(),
                    decoderRenderer.getQueueDelayMs())) {
                LimeLog.warning("Automatic bitrate: a rate change could not be sent; off for this stream");
                messages.alert(R.string.stream_rate_change_auto_not_sent, Toast.LENGTH_LONG);
                return;
            }
            scheduleRateChangeExpiry();
            bitrateHandler.postDelayed(this, 1000);
        }
    };
    // Gives up on 0x553C requests the host left unanswered for StreamRateChanges.ANSWER_MS, loudly.
    private final Runnable expireRateChanges = new Runnable() {
        @Override public void run() {
            if (!connected || rateChanges == null) return;
            java.util.EnumSet<StreamRateChanges.Answer> expired = rateChanges.expire(SystemClock.uptimeMillis());
            if (expired.contains(StreamRateChanges.Answer.AUTOMATIC_NO_ANSWER)) {
                bitrateHandler.removeCallbacks(updateRateChangeBitrate);
                LimeLog.warning("Automatic bitrate: the host did not answer a rate change; off for this stream");
                messages.alert(R.string.stream_rate_change_auto_no_answer, Toast.LENGTH_LONG);
            }
            if (expired.contains(StreamRateChanges.Answer.MANUAL_NO_ANSWER)) {
                messages.alert(R.string.stream_rate_change_no_answer, Toast.LENGTH_LONG);
            }
            scheduleRateChangeExpiry();
        }
    };
    private final Runnable sampleStreamHealth = new Runnable() {
        @Override public void run() {
            if (!connected || decoderRenderer == null || app.getRole() == NvApp.Role.INPUT_ONLY) return;
            if (foreground && !hostActionInProgress) {
                // A hanging decoder holds the locks this sample reads for seconds: never on the main thread.
                MediaCodecDecoderRenderer renderer = decoderRenderer;
                int fps = streamFps;
                if (!streamHealthSampler().sample(() -> {
                    long rtt = MoonBridge.getEstimatedRttInfo();
                    return renderer.healthSample(SystemClock.uptimeMillis(), fps,
                            rtt == -1 ? -1 : (float) (rtt & 0xffffffffL));
                }, streamHealth::add)) {
                    renderer.skipHealthSample();
                    LimeLog.info("Stream health: the last sample is still running; this one is skipped");
                }
            } else {
                decoderRenderer.skipHealthSample();
            }
            bitrateHandler.postDelayed(this, 1000);
        }
    };
    private java.util.concurrent.ExecutorService streamHealthThread;
    private com.limelight.utils.StreamHealthSampler streamHealthSampler;

    private com.limelight.utils.StreamHealthSampler streamHealthSampler() {
        if (streamHealthSampler == null) {
            streamHealthThread = java.util.concurrent.Executors.newSingleThreadExecutor(task -> {
                Thread thread = new Thread(task, "Stream health");
                thread.setDaemon(true);
                return thread;
            });
            streamHealthSampler = new com.limelight.utils.StreamHealthSampler(streamHealthThread, bitrateHandler::post);
        }
        return streamHealthSampler;
    }

    private final Runnable updateQuality = new Runnable() {
        @Override public void run() {
            if (!connected || adaptiveQuality == null || isFinishing()) return;
            long now = SystemClock.uptimeMillis();
            if (!foreground || streamMenu != null || hostActionInProgress ||
                    (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && isInPictureInPictureMode())) {
                adaptiveQuality.suspend();
                decoderRenderer.resetDecodeTiming();
            } else if (pendingQualityTarget != null) {
                decoderRenderer.resetDecodeTiming();
                int[] shown = decoderRenderer.getStreamSize();
                if (shown[0] == pendingQualityTarget.width && shown[1] == pendingQualityTarget.height) {
                    pendingQualityTarget = null;
                } else if (now - qualityRequestSentAt >= 10000) {
                    stopAdaptiveQualityWithError();
                    return;
                }
            } else {
                long rtt = MoonBridge.getEstimatedRttInfo();
                AdaptiveQualityController.Target target = adaptiveQuality.sample(now,
                        decoderRenderer.hasRecentVideoFrames(now), poorConnection,
                        decoderRenderer.getNetworkFrameLossPercent(), decoderRenderer.getDecodeTimeMs(),
                        rtt == -1 ? -1 : (float) (rtt & 0xffffffffL), decoderRenderer.getQueueDelayMs(),
                        adaptationThermalLevel(),
                        adaptiveBitrate != null && adaptiveBitrate.hasRoomBeforeQualityReduction(),
                        decoderRenderer.takeDecodeTiming(streamFps));
                if (target != null && !applyAdaptiveQuality(target, now)) return;
            }
            bitrateHandler.postDelayed(this, 1000);
        }
    };
    // Once a second: this second's loss and frame rate for a Foundation PC's controller, and the
    // report every three seconds (FoundationAbr).
    private final Runnable foundationAbrTick = new Runnable() {
        @Override
        public void run() {
            com.limelight.nvstream.http.FoundationAbr abr = foundationAbr;
            if (!connected || abr == null) return;
            float[] rates = decoderRenderer.getFrameRates();
            abr.add(decoderRenderer.getNetworkFrameLossPercent(), rates.length > 1 ? Math.max(0, rates[1]) : 0);
            long rttInfo = MoonBridge.getEstimatedRttInfo();
            org.json.JSONObject feedback = abr.due(SystemClock.uptimeMillis(), rttInfo == -1 ? 0 : rttInfo >>> 32);
            if (feedback != null) {
                NvConnection connection = conn;
                new Thread(() -> {
                    org.json.JSONObject action = null;
                    try {
                        action = connection.sendFoundationAbrFeedback(feedback);
                    } catch (IOException e) {
                        LimeLog.warning("The PC's automatic bitrate report failed: " + e.getMessage());
                    }
                    org.json.JSONObject answer = action;
                    runOnUiThread(() -> {
                        if (foundationAbr != abr) return;
                        int kbps = abr.answered(answer);
                        if (kbps > 0) {
                            LimeLog.info("The PC set " + kbps + " kbps: " + answer.optString("reason"));
                            currentBitrate = kbps;
                        }
                    });
                }, "FoundationAbr").start();
            }
            bitrateHandler.postDelayed(this, 1000);
        }
    };
    /** The heat the bitrate and quality controllers react to: none unless Thermal protection is on. */
    private int adaptationThermalLevel() {
        return com.limelight.binding.video.ThermalMonitor.forAdaptation(prefConfig.thermalProtection,
                decoderRenderer.getThermalLevel());
    }

    private final Runnable updateBitrate = new Runnable() {
        @Override
        public void run() {
            if (!connected || !isAdaptiveBitrateEnabled()) {
                return;
            }
            if (!hostActionInProgress && streamMenu == null && SystemClock.uptimeMillis() >= adaptiveBitrateRetryAt) {
                if (adaptiveBitrateRetryAt != 0) {
                    applyAdaptiveBitrate(adaptiveBitrateRetryTarget);
                } else {
                    long now = SystemClock.uptimeMillis();
                    long rttInfo = MoonBridge.getEstimatedRttInfo();
                    float jitterMs = rttInfo == -1 ? -1 : (float) (rttInfo & 0xffffffffL);
                    int thermalLevel = adaptationThermalLevel();
                    if (adaptiveBitrate != null) adaptiveBitrate.setThermalLevel(thermalLevel);
                    if (pyroWaveBitrate != null) pyroWaveBitrate.setThermalLevel(thermalLevel);
                    int target = pyroWaveBitrate != null ?
                            pyroWaveBitrate.sample(now, decoderRenderer.hasRecentVideoFrames(now), poorConnection,
                                    decoderRenderer.getPyroWaveLossPercent(), decoderRenderer.getPyroWaveQueueDelayMs(),
                                    decoderRenderer.getDecodeTimeMs(), jitterMs) :
                            adaptiveBitrate.sample(now, decoderRenderer.hasRecentVideoFrames(now),
                                    poorConnection, decoderRenderer.getNetworkFrameLossPercent(), decoderRenderer.getDecodeTimeMs(), jitterMs,
                                    decoderRenderer.getQueueDelayMs());
                    if (target != 0) {
                        if (pyroWaveBitrate == null) {
                            LimeLog.info("Device controls bitrate: " + currentBitrate + " -> " + target + " kbps (" +
                                    adaptiveBitrate.lastReason() + ")");
                        }
                        applyAdaptiveBitrate(target);
                    }
                }
            } else {
                if (adaptiveBitrate != null) adaptiveBitrate.suspend();
                if (pyroWaveBitrate != null) pyroWaveBitrate.suspend();
            }
            bitrateHandler.postDelayed(this, 1000);
        }
    };
    private String pcName;
    private String appName;
    NvApp app;
    private float desiredRefreshRate;
    private float panelMaxRefreshRate;
    // Mid-stream reconfigure (0x5532): a stream at the phone's native resolution, or at the
    // panel's top refresh rate, follows fold, window and panel changes without reconnecting.
    private boolean nativeResolutionStream;
    private boolean fpsFollowsDisplay;
    private int streamFps;
    private float streamPanelHz;
    private boolean streamStartedLandscape = true;
    // The Orientation setting, and whether the host draws this stream at the shape asked for (its
    // virtual display), so a turned window can ask for a turned stream.
    private int streamOrientation = StreamOrientation.AUTO;
    private boolean hostShapesStream;
    private int launchWidth, launchHeight;
    // The screen's own resolution when the stream was asked for; null until then.
    private String launchPanel;
    private int lastWindowOrientation = Configuration.ORIENTATION_UNDEFINED;
    private boolean orientationNoticeShown;
    private String displaySignature = "";
    private int lastPaneWidth, lastPaneHeight;
    private final StreamReconfigureRequester reconfigureRequester = new StreamReconfigureRequester();
    private final Runnable checkReconfigure = this::checkStreamReconfigure;
    private final android.hardware.display.DisplayManager.DisplayListener inputDisplayListener =
            new android.hardware.display.DisplayManager.DisplayListener() {
                @Override public void onDisplayAdded(int displayId) {}
                @Override public void onDisplayRemoved(int displayId) {
                    Display video = getVideoDisplay();
                    if (video.getDisplayId() == displayId) noteVideoDisplay(video, true);
                    if (externalPresentation != null && externalPresentation.getDisplay().getDisplayId() == displayId) {
                        returnVideoToDevice();
                    }
                }
                @Override public void onDisplayChanged(int displayId) {
                    Display display = getVideoDisplay();
                    if (display.getDisplayId() == displayId) noteVideoDisplay(display, false);
                    if (conn != null && display.getDisplayId() == displayId) {
                        updateInputPollingRate();
                        // Also fires when a foldable moves the stream to its other screen.
                        reportDisplayCaps();
                        onDisplayModesChanged();
                    }
                }
            };
    // Whether this stream asked for HDR (setting on, HDR10 display and decoder).
    private boolean streamHdr;
    // 10-bit formats offered for 10-bit SDR (the setting), without asking the PC for HDR.
    private boolean streamSdr10;
    private Boolean sdr10LastHostHdr;
    private HdrPolicy.Status lastHdrStatus = HdrPolicy.Status.QUIET;
    private DisplayCapsReporter displayCapsReporter;

    // Asks a host that sends per-frame timing reports (0x5536) for them, once per connection.
    // Feeds the overlay's host latency line and the latency CSV; the stream description's
    // announcement is the final say, the capability only keeps other hosts from being asked.
    private void requestHostTiming() {
        if (conn == null || !conn.getHostDetails().caps.has(HostCapabilities.Feature.HOST_TIMING) ||
                !MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_HOST_TIMING)) {
            return;
        }
        MoonBridge.sendHostTimingRequest(true);
    }

    // Asks a host that announces link notices (0x5538) to say when it lowers the video bitrate
    // because of the network, once per connection. The overlay then shows it until the host is
    // back at the requested rate. The C engine does not support it.
    private void requestLinkNotice() {
        linkNotice = null;
        if (conn == null || !MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_LINK_NOTICE)) {
            return;
        }
        MoonBridge.sendLinkNoticeRequest(true);
    }

    // Clipboard sync (0x5508), when this stream's launch asked a Rubylight PC for it. The C engine has none.
    private void startClipboardSync() {
        if (clipboardSync != null) {
            clipboardSync.stop();
            clipboardSync = null;
        }
        if (conn == null || !conn.askedForClipboardSync()) return;
        if (!MoonBridge.isRustStream()) {
            LimeLog.warning("Clipboard sync: needs the New streaming engine; this stream has none");
            return;
        }
        clipboardSync = com.limelight.utils.ClipboardSync.forStream(this,
                text -> messages.alert(text, Toast.LENGTH_LONG));
        clipboardSync.connectionStarted(hasWindowFocus());
    }

    // Reports the output's HDR capabilities to Rubylight 2.2.0+ hosts. Keep luminance fixed on
    // that output: brightness changes must not reconfigure HDR on older hosts.
    private void reportDisplayCaps() {
        if (!connected || displayCapsReporter == null ||
                !MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_DISPLAY_CAPS)) {
            return;
        }
        Display display = getVideoDisplay();
        boolean displayHdr = false;
        float maxNits = 0, maxAverageNits = 0, minNits = 0;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            // getHdrCapabilities() returns null on some devices (Lenovo Mirage Solo, Android 8.0).
            Display.HdrCapabilities hdrCaps = display.getHdrCapabilities();
            if (hdrCaps != null) {
                // HLG alone makes this an HDR output only while the stream is HLG: never for a PQ stream.
                displayHdr = HdrPolicy.reportsHdrOutput(hdrCaps.getSupportedHdrTypes(), decoderRenderer != null ?
                        decoderRenderer.getStreamTransfer() : MediaFormat.COLOR_TRANSFER_ST2084);
                maxNits = hdrCaps.getDesiredMaxLuminance();
                maxAverageNits = hdrCaps.getDesiredMaxAverageLuminance();
                minNits = hdrCaps.getDesiredMinLuminance();
            }
        }
        String output = Integer.toString(display.getDisplayId());
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            Display.Mode mode = display.getMode();
            // Foldable panels can share a logical display ID. Pixel geometry distinguishes them
            // without reacting to brightness, rotation, or an automatic refresh-rate change.
            output += ":" + Math.min(mode.getPhysicalWidth(), mode.getPhysicalHeight()) + "x" +
                    Math.max(mode.getPhysicalWidth(), mode.getPhysicalHeight());
        }
        DisplayCapsReporter.Caps sent = displayCapsReporter.update(output, streamHdr && displayHdr,
                maxNits, maxAverageNits, minNits, hdrSdrRatio(display));
        if (sent != null) {
            LimeLog.info("Display luminance sent to host: " + sent);
        }
    }

    // Android's HDR/SDR ratio (API 34+; the larger of the current and highest on Android 16), for
    // an output that reports no peak. NaN when there is none.
    private static float hdrSdrRatio(Display display) {
        if (Build.VERSION.SDK_INT < 34 || !display.isHdrSdrRatioAvailable()) {
            return Float.NaN;
        }
        float current = display.getHdrSdrRatio();
        float highest = Build.VERSION.SDK_INT >= 36 ? display.getHighestHdrSdrRatio() : current;
        return HdrBrightness.bestHdrSdrRatio(current, highest);
    }

    // The output's luminance for the launch query of hosts other than Rubylight (Rubylight takes
    // the DisplayCaps control message instead), as Moonlight V+ sends it. Null when unknown.
    static HdrBrightness launchLuminance(Display display) {
        float maxNits = Float.NaN, maxAverageNits = Float.NaN, minNits = Float.NaN;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            Display.HdrCapabilities hdrCaps = display.getHdrCapabilities();
            if (hdrCaps != null) {
                maxNits = hdrCaps.getDesiredMaxLuminance();
                maxAverageNits = hdrCaps.getDesiredMaxAverageLuminance();
                minNits = hdrCaps.getDesiredMinLuminance();
            }
        }
        return HdrBrightness.fromDisplay(maxNits, minNits, maxAverageNits, hdrSdrRatio(display));
    }

    private void updateInputPollingRate() {
        Display display = getWindowManager().getDefaultDisplay();
        conn.setInputPollingRate(Build.VERSION.SDK_INT >= Build.VERSION_CODES.M ?
                display.getMode().getRefreshRate() : display.getRefreshRate());
    }
    boolean useArr;
    private final DisplayRefreshDiagnostics displayRefreshDiagnostics = new DisplayRefreshDiagnostics();
    // The last successful vote made by this activity, not a measurement of the display's rate.
    private Float appSurfaceFrameRateVote;
    // Follows the content rate measured from the host's frame times (checkbox_content_rate_frame_rate).
    private final com.limelight.binding.video.DisplayFrameRatePolicy.ContentRateVote contentRateVote =
            new com.limelight.binding.video.DisplayFrameRatePolicy.ContentRateVote();
    // Flags ARR showing about half the rendered frames (see checkArrRate).
    private final com.limelight.binding.video.DisplayFrameRatePolicy.ArrHalfRateCheck arrHalfRate =
            new com.limelight.binding.video.DisplayFrameRatePolicy.ArrHalfRateCheck();
    // Whether the vote above allowed only seamless switches (the content rate vote, and every vote on API 30).
    private boolean appSurfaceSeamlessOnly;
    // Tells why the screen did not follow the vote (see RefreshSwitchDiagnosis); created once connected.
    private com.limelight.binding.video.RefreshSwitchMonitor refreshSwitchMonitor;
    private final Runnable pollRefreshSwitch = new Runnable() {
        @Override public void run() {
            if (!connected || isFinishing()) return;
            checkRefreshSwitch();
            checkArrRate();
            checkHdrStatus();
            checkVulkanPace();
            bitrateHandler.postDelayed(this, 1000);
        }
    };
    private final Runnable pollContentRateVote = new Runnable() {
        @Override public void run() {
            if (!connected || isFinishing()) return;
            voteContentRate();
            bitrateHandler.postDelayed(this, 1000);
        }
    };
    private PendingIntent pipDisconnectIntent;
    private final BroadcastReceiver pipReceiver = new BroadcastReceiver() {
        @Override public void onReceive(Context context, Intent intent) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
                    PictureInPicturePolicy.canDisconnect(connected, isInPictureInPictureMode(), app.getRole())) {
                leaveStream();
            }
        }
    };
    InputCaptureProvider inputCaptureProvider;
    int modifierFlags = 0;
    boolean grabbedInput = true;
    boolean cursorVisible = false;
    StreamView streamView;
    private StreamView renderView;
    com.limelight.ui.StreamPresentation externalPresentation;
    com.limelight.ui.DesktopGestures desktopGestures;
    com.limelight.ui.GameTouchControls gameTouchControls;
    private boolean outputSwitchPending;
    private static final String EXTRA_OUTPUT_DISPLAY = "rubylight.outputDisplay";
    private static final String EXTRA_OUTPUT_RESUME_ONLY = "rubylight.outputResumeOnly";
    private Surface videoSurface;

    boolean isHidingOverlays;
    private boolean wasInPictureInPicture;
    TextView notificationOverlayView;
    private int requestedNotificationOverlayVisibility = View.GONE;
    TextView performanceOverlayView;
    View performanceOverlayScroll;
    // What the host last said about holding the bitrate down for the network; null when it has not.
    LinkNoticeStatus linkNotice;
    // The soft keyboard below Android 11, which tells an adjustNothing window nothing of it.
    LegacyKeyboardInsets legacyKeyboard;
    final Handler keyboardPanHandler = new Handler(Looper.getMainLooper());
    final Runnable keyboardPan = keyboardInput::updateKeyboardPan;
    // PerformanceOverlay.MODE_COMPACT, MODE_STATISTICS or MODE_DEBUG
    String overlayMode = PerformanceOverlay.MODE_COMPACT;
    // The corner the overlay prefers; OverlayPlacement moves it when controls cover that corner.
    com.limelight.ui.OverlayPlacement.Corner overlayCorner = com.limelight.ui.OverlayPlacement.Corner.TOP_LEFT;
    TextView streamMenuHint;
    // Every message of the stream goes through here: one at a time, lasting facts once per PC.
    final com.limelight.ui.StreamMessages messages = new com.limelight.ui.StreamMessages(this);
    // From the first time the user opens USB devices until the stream stops.
    private com.limelight.binding.usb.UsbForwarding usbForwarding;
    // Clipboard sync (0x5508) while this stream's PC runs it.
    private com.limelight.utils.ClipboardSync clipboardSync;
    final Runnable showStreamMenuHint = overlays::showStreamMenuHint;

    MediaCodecDecoderRenderer decoderRenderer;
    private boolean reportedCrash;
    // The PC (or this device) ended the stream. A decoder failure after that is the stream going
    // away, not a decoder crash.
    private volatile boolean streamTerminated;
    // Experimental: a TV that loses its HDMI signal takes the decoder down with it. Whether the
    // video display is off, invalid or gone, as of its last change.
    private volatile boolean videoDisplayOff;
    private volatile Boolean androidTv;

    private boolean isAndroidTv() {
        Boolean tv = androidTv;
        if (tv == null) {
            tv = getPackageManager().hasSystemFeature(PackageManager.FEATURE_TELEVISION) ||
                    getPackageManager().hasSystemFeature(PackageManager.FEATURE_LEANBACK);
            androidTv = tv;
        }
        return tv;
    }

    private boolean streamEnding() {
        // hostExitRequested is left out: a failed quit keeps it set for 15 s while still streaming.
        return streamTerminated || exiting || connectionStopThread != null;
    }

    private void noteVideoDisplay(Display display, boolean removed) {
        videoDisplayOff = removed || !display.isValid() || display.getState() != Display.STATE_ON;
        stagedExit.onDisplayChanged(SystemClock.uptimeMillis());
    }

    // Experimental: on Android TV, an HDR stream ends in stages (stop and release the decoder and
    // video output, let the display settle, give the display mode back, settle again, then finish)
    // so the HDR to SDR switch and the mode revert never overlap and wedge the TV's HDMI link.
    // Everything else finishes at once, as always. See StagedStreamExit.
    private final StagedStreamExit stagedExit = new StagedStreamExit();
    private final Handler exitHandler = new Handler(Looper.getMainLooper());
    private final Runnable stagedExitRunnable = this::stagedExitTick;
    // True from the first finish() of a staged exit on; isFinishing() reports it so every
    // "is this stream ending" check in this class holds while the exit is staged.
    private volatile boolean exiting;
    private boolean exitFinished;
    private boolean stagedExitStopDone;

    @Override
    public boolean isFinishing() {
        return exiting || super.isFinishing();
    }

    @Override
    public void finish() {
        if (exitFinished) {
            super.finish();
        } else if (exiting) {
            LimeLog.info("Exit: already in progress, ignoring finish()");
        } else if (stagedExitApplies()) {
            beginStagedExit();
        } else {
            exitFinished = true;
            super.finish();
        }
    }

    // For a hand-off to a new stream activity (reconnect, new intent): the stream is not ending.
    private void finishNow() {
        exitFinished = true;
        super.finish();
    }

    private boolean stagedExitApplies() {
        return !isDestroyed() && attemptedConnection && decoderRenderer != null &&
                StagedStreamExit.applies(isAndroidTv(),
                        streamHdr || Boolean.TRUE.equals(decoderRenderer.getHostHdrReport()));
    }

    private void beginStagedExit() {
        if (!stagedExit.begin(SystemClock.uptimeMillis())) return;
        exiting = true;
        LimeLog.info("Exit: staged (Android TV, HDR stream, Experimental)");
        touchInput.setInputGrabState(false);
        if (streamMenu != null) streamMenu.dismiss();
        LimeLog.info("Exit: stopping the connection and releasing the decoder");
        stopConnection();
        exitHandler.post(stagedExitRunnable);
    }

    private void stagedExitTick() {
        if (exitFinished || isDestroyed()) return;
        long now = SystemClock.uptimeMillis();
        Thread stopping = connectionStopThread;
        if ((stopping == null || !stopping.isAlive()) && !stagedExitStopDone) {
            stagedExitStopDone = true;
            stagedExit.onStopFinished();
            if (stagedExit.stopTimedOut()) LimeLog.info("Exit: connection stop finished late");
        }
        switch (stagedExit.poll(now)) {
            case RELEASE_OUTPUT:
                LimeLog.info(stagedExit.stopTimedOut() ?
                        "Exit: connection stop still running after " + StagedStreamExit.STOP_TIMEOUT_MS +
                                " ms, releasing the video output anyway and waiting for it before the mode restore" :
                        "Exit: decoder released, releasing the video output");
                releaseVideoOutput();
                break;
            case RESTORE_DISPLAY_MODE:
                LimeLog.info("Exit: display settled, restoring the display mode");
                restoreDisplayMode();
                break;
            case FINISH:
                LimeLog.info("Exit: display settled, finishing");
                exitFinished = true;
                super.finish();
                return;
            default:
                break;
        }
        exitHandler.postDelayed(stagedExitRunnable, StagedStreamExit.POLL_MS);
    }

    // Tears the video surface down, which ends the HDR output. The stop thread has released
    // the decoder by now (or timed out; the surface callbacks then stop it as they always do).
    private void releaseVideoOutput() {
        // Where the display is now stays requested until RESTORE_DISPLAY_MODE: on Android 12+ the
        // refresh rate may have been switched by the surface's frame-rate vote, which would
        // otherwise be dropped together with the HDR output.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            try {
                android.view.Window window = getVideoWindow();
                WindowManager.LayoutParams attributes = window.getAttributes();
                attributes.preferredDisplayModeId = getVideoDisplay().getMode().getModeId();
                window.setAttributes(attributes);
            } catch (RuntimeException e) {
                LimeLog.warning("Exit: could not pin the display mode: " + e);
            }
        }
        if (streamView != null) streamView.setVisibility(View.GONE);
        if (renderView != null && renderView != streamView) renderView.setVisibility(View.GONE);
    }

    // Gives back what prepareDisplayForRendering() and the pin above asked of the display, instead
    // of leaving it to the window going away at the same time as something else. The surface is
    // gone by now, so its frame-rate vote went with it.
    private void restoreDisplayMode() {
        try {
            android.view.Window window = getVideoWindow();
            WindowManager.LayoutParams attributes = window.getAttributes();
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) attributes.preferredDisplayModeId = 0;
            attributes.preferredRefreshRate = 0;
            window.setAttributes(attributes);
        } catch (RuntimeException e) {
            LimeLog.warning("Exit: could not restore the display mode: " + e);
        }
    }

    private WifiManager.WifiLock highPerfWifiLock;
    private WifiManager.WifiLock lowLatencyWifiLock;

    private boolean usbDriverServiceBound;
    private UsbDriverService.UsbDriverBinder usbDriverBinder;
    private ServiceConnection usbDriverServiceConnection = new ServiceConnection() {
        @Override
        public void onServiceConnected(ComponentName componentName, IBinder iBinder) {
            if (isFinishing() || isDestroyed() || connectionStopThread != null) {
                return;
            }
            UsbDriverService.UsbDriverBinder binder = (UsbDriverService.UsbDriverBinder) iBinder;
            usbDriverBinder = binder;
            binder.setListener(controllerHandler);
            binder.setStateListener(Game.this);
            binder.start();
        }

        @Override
        public void onServiceDisconnected(ComponentName componentName) {
            usbDriverBinder = null;
        }
    };

    public static final String EXTRA_HOST = "Host";
    public static final String EXTRA_PORT = "Port";
    public static final String EXTRA_HTTPS_PORT = "HttpsPort";
    public static final String EXTRA_APP_NAME = "AppName";
    public static final String EXTRA_APP_ID = "AppId";
    public static final String EXTRA_APP_UUID = "AppUuid";
    public static final String EXTRA_MONITOR_RESUME = "MonitorResume";
    public static final String EXTRA_SMALL_STREAM_INPUT_ONLY = "SmallStreamInputOnly";
    public static final String EXTRA_UNIQUEID = "UniqueId";
    public static final String EXTRA_PC_UUID = "UUID";
    public static final String EXTRA_PC_NAME = "PcName";
    /** The running game this start replaces, named in the host's confirmation prompt. */
    public static final String EXTRA_REPLACED_APP_NAME = "ReplacedAppName";
    public static final String EXTRA_APP_HDR = "HDR";
    public static final String EXTRA_SERVER_CERT = "ServerCert";
    private static final String EXTRA_RECOVERY_ATTEMPTS = "RecoveryAttempts";
    // The network the stream started on (StreamLayers.Network), kept across reconnects and handoffs.
    private static final String EXTRA_STREAM_NETWORK = "StreamNetwork";
    private static final String EXTRA_RECOVERY_STARTED = "RecoveryStarted";
    private static final String EXTRA_RECOVERY_MEDIA_FAILURE = "RecoveryMediaFailure";
    // An earlier connection of this automatic reconnect chain showed video (videoStarted).
    private static final String EXTRA_RECOVERY_VIDEO = "RecoveryVideoShown";
    // Packed full chroma failed in this session and the reconnect runs without it (not the setting).
    private static final String EXTRA_PACKED_OFF = "PackedChromaOff";
    private static final String EXTRA_MICROPHONE_MUTED = "rubylight.microphoneMuted";
    // On a resume after the user's own trip away: the failure budget {attempts, started, left at,
    // connected for} as the trip began, which the resumed activity starts with.
    private static final String EXTRA_TRIP_BUDGET = "rubylight.tripBudget";
    // When the user returned from that trip: marks the resume and starts its own deadline.
    private static final String EXTRA_TRIP_RESUME_STARTED = "rubylight.tripResumeStarted";

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        com.limelight.ui.AppTheme.apply(this);
        super.onCreate(savedInstanceState);

        if (savedInstanceState != null) {
            LimeLog.info("Stream restored after process death");
            messages.alert(R.string.stream_restore_help, Toast.LENGTH_LONG);
            finish();
            return;
        }
        if (!isValidLaunch(getIntent().getStringExtra(EXTRA_HOST),
                getIntent().getIntExtra(EXTRA_PORT, NvHTTP.DEFAULT_HTTP_PORT),
                getIntent().getStringExtra(EXTRA_UNIQUEID),
                getIntent().getIntExtra(EXTRA_APP_ID, StreamConfiguration.INVALID_APP_ID),
                getIntent().getStringExtra(EXTRA_APP_UUID))) {
            finish();
            return;
        }

        UiHelper.setLocale(this);

        // We don't want a title bar
        requestWindowFeature(Window.FEATURE_NO_TITLE);

        // Full-screen
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN);

        // If we're going to use immersive mode, we want to have
        // the entire screen
        getWindow().getDecorView().setSystemUiVisibility(
                View.SYSTEM_UI_FLAG_LAYOUT_STABLE |
                        View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION |
                        View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN);

        getWindow().addFlags(WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN);

        // Listen for UI visibility events
        getWindow().getDecorView().setOnSystemUiVisibilityChangeListener(this);

        // Change volume button behavior
        setVolumeControlStream(AudioManager.STREAM_MUSIC);

        // Inflate the content
        setContentView(R.layout.activity_game);

        // Start the spinner
        spinner = SpinnerDialog.displayDialog(this, getResources().getString(R.string.conn_establishing_title),
                getResources().getString(R.string.conn_establishing_msg), true);

        // Read the stream preferences
        // Profiles, PC and game values are read once here and never change during the stream. A
        // reconnect or handoff keeps the network the stream started on, and so the same profile.
        com.limelight.preferences.StreamLayers.Network startNetwork = null;
        String seenNetwork = getIntent().getStringExtra(EXTRA_STREAM_NETWORK);
        if (seenNetwork != null) {
            try {
                startNetwork = com.limelight.preferences.StreamLayers.Network.valueOf(seenNetwork);
            } catch (IllegalArgumentException ignored) {
                // Read the network again.
            }
        }
        prefConfig = PreferenceConfiguration.readPreferences(this, getIntent().getStringExtra(EXTRA_PC_UUID),
                HostCapabilityCache.load(this, getIntent().getStringExtra(EXTRA_PC_UUID)),
                getIntent().getStringExtra(EXTRA_APP_UUID),
                getIntent().getIntExtra(EXTRA_APP_ID, StreamConfiguration.INVALID_APP_ID), startNetwork);
        prefConfig.experimentalPacked444 = PackedFallback.effective(prefConfig.experimentalPacked444,
                getIntent().getBooleanExtra(EXTRA_PACKED_OFF, false));
        getIntent().putExtra(EXTRA_STREAM_NETWORK, prefConfig.layers.network.name());
        HdrPolicy.setDebugHlgOnly(BuildConfig.DEBUG, prefConfig.debugHdrHlgOnly);
        overlays.showStreamLayers(prefConfig, startNetwork == null);
        readRecoveryBudget(SystemClock.elapsedRealtime());
        panelMaxRefreshRate = com.limelight.binding.video.DisplayFrameRatePolicy.maxRefreshRate(
                getVideoDisplay());
        if (prefConfig.vrr) {
            prefConfig.fps = com.limelight.binding.video.DisplayFrameRatePolicy.streamFrameRate(panelMaxRefreshRate);
            prefConfig.launchRefreshRateX100 = Math.max(100, Math.min(100000, Math.round(panelMaxRefreshRate * 100)));
        }
        com.limelight.utils.RedactedLog.addSecret(getIntent().getStringExtra(EXTRA_PC_NAME));
        com.limelight.utils.RedactedLog.addSecret(getIntent().getStringExtra(EXTRA_HOST));
        LimeLog.info(LaunchSource.describe(getIntent()));
        LimeLog.info("Stream request: " + prefConfig.width + "x" + prefConfig.height + " at " + prefConfig.fps +
                " fps (resolution " + prefConfig.resolutionSource() +
                (prefConfig.vrr ? ", VRR at the panel's " + panelMaxRefreshRate + " Hz" : "") + ")");
        LimeLog.info("PC's own stream values: " + (prefConfig.pcSettings == null ? "none" : prefConfig.pcSettings));
        if (prefConfig.layers != null) {
            LimeLog.info("Stream settings layers: " + com.limelight.preferences.StreamLayers.describe(prefConfig.layers));
        }
        // Fixed presets keep their size; anything else is the phone's own resolution and follows it.
        nativeResolutionStream = prefConfig.resolutionFollowsScreen ||
                PreferenceConfiguration.isNativeResolution(prefConfig.width, prefConfig.height);
        fpsFollowsDisplay = prefConfig.vrr ||
                prefConfig.fps == com.limelight.binding.video.DisplayFrameRatePolicy.streamFrameRate(panelMaxRefreshRate);
        nativeTouchEnabled = !prefConfig.touchscreenTrackpad &&
                PreferenceManager.getDefaultSharedPreferences(this).getBoolean("checkbox_native_touch", false);
        tombstonePrefs = Game.this.getSharedPreferences("DecoderTombstone", 0);
        streamOrientation = prefConfig.streamOrientation;
        lastWindowOrientation = getResources().getConfiguration().orientation;

        // Landscape on phones unless the Orientation setting says otherwise
        setPreferredOrientationForCurrentDisplay();

        configureStreamWindow();

        // Listen for non-touch events on the game surface
        streamView = findViewById(R.id.surfaceView);
        initializeVideoOutput();
        streamView.setOnGenericMotionListener(this);
        // Keeps the Local cursor pointer over the same stream pixel when the stream moves or resizes.
        streamView.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> {
            if (trackpadPointer != null) trackpadPointer.syncCursor();
        });
        streamView.setOnKeyListener(this);
        streamView.setInputCallbacks(this);
        adaptiveLayout = com.limelight.ui.AdaptiveLayout.attach(this, findViewById(android.R.id.content));
        adaptiveLayout.setInsetsObserver(keyboardInput::onWindowInsets);
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            legacyKeyboard = new LegacyKeyboardInsets(this, findViewById(android.R.id.content), keyboardInput::onKeyboardInsets);
            legacyKeyboard.start();
        }
        streamView.addOnLayoutChangeListener((view, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom) -> {
            if (connected) updatePipAutoEnter();
        });
        findViewById(android.R.id.content).addOnLayoutChangeListener(
                (view, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom) -> onPaneLaidOut(view));

        // Listen for touch events on the background touch view to enable trackpad mode
        // to work on areas outside of the StreamView itself. We use a separate View
        // for this rather than just handling it at the Activity level, because that
        // allows proper touch splitting, which the OSC relies upon.
        View backgroundTouchView = findViewById(R.id.backgroundTouchView);
        backgroundTouchView.setOnTouchListener(this);

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            // Request unbuffered input event dispatching for all input classes we handle here.
            // Without this, input events are buffered to be delivered in lock-step with VBlank,
            // artificially increasing input latency while streaming.
            streamView.requestUnbufferedDispatch(
                    InputDevice.SOURCE_CLASS_BUTTON | // Keyboards
                    InputDevice.SOURCE_CLASS_JOYSTICK | // Gamepads
                    InputDevice.SOURCE_CLASS_POINTER | // Touchscreens and mice (w/o pointer capture)
                    InputDevice.SOURCE_CLASS_POSITION | // Touchpads
                    InputDevice.SOURCE_CLASS_TRACKBALL // Mice (pointer capture)
            );
            backgroundTouchView.requestUnbufferedDispatch(
                    InputDevice.SOURCE_CLASS_BUTTON | // Keyboards
                    InputDevice.SOURCE_CLASS_JOYSTICK | // Gamepads
                    InputDevice.SOURCE_CLASS_POINTER | // Touchscreens and mice (w/o pointer capture)
                    InputDevice.SOURCE_CLASS_POSITION | // Touchpads
                    InputDevice.SOURCE_CLASS_TRACKBALL // Mice (pointer capture)
            );
        }

        notificationOverlayView = findViewById(R.id.notificationOverlay);
        streamMenuHint = findViewById(R.id.streamMenuHint);
        // Rotation, a fold or the overlay moving to the top centre move the hint too.
        streamMenuHint.getViewTreeObserver().addOnGlobalLayoutListener(() -> {
            if (streamMenuHint.getVisibility() == View.VISIBLE) {
                com.limelight.ui.StreamMenuHint.place(streamMenuHint, performanceOverlayScroll);
            }
        });

        performanceOverlayView = findViewById(R.id.performanceOverlay);
        performanceOverlayScroll = findViewById(R.id.performanceOverlayScroll);
        performanceOverlayScroll.setOnTouchListener((view, event) -> !overlays.isDebugOverlay() && touchInput.onOverlayTouch(view, event));
        overlayMode = PerformanceOverlay.normalizeMode(PreferenceManager.getDefaultSharedPreferences(this)
                .getString("performance_overlay_mode", PerformanceOverlay.MODE_COMPACT));
        overlayCorner = com.limelight.ui.OverlayPlacement.Corner.parse(prefConfig.overlayPosition);
        overlays.attachOverlayPlacement((android.view.ViewGroup) streamView.getParent());
        overlays.updatePerformanceOverlay();

        inputCaptureProvider = InputCaptureManager.getInputCaptureProvider(this, this);

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            streamView.setOnCapturedPointerListener(new View.OnCapturedPointerListener() {
                @Override
                public boolean onCapturedPointer(View view, MotionEvent motionEvent) {
                    return touchInput.handleMotionEvent(view, motionEvent);
                }
            });
        }

        // Warn the user if they're on a metered connection
        ConnectivityManager connMgr = (ConnectivityManager) getSystemService(Context.CONNECTIVITY_SERVICE);
        if (connMgr.isActiveNetworkMetered() && !prefConfig.disableWarnings) {
            // Streaming can use a lot of mobile data: worth acting on before it does.
            messages.alert(R.string.conn_metered, Toast.LENGTH_LONG);
        }

        // Make sure Wi-Fi is fully powered up
        WifiManager wifiMgr = (WifiManager) getApplicationContext().getSystemService(Context.WIFI_SERVICE);
        try {
            if (wifiMgr != null) {
                highPerfWifiLock = wifiMgr.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, BuildConfig.BRAND_NAME + " High Perf Lock");
                highPerfWifiLock.setReferenceCounted(false);
                highPerfWifiLock.acquire();

                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    lowLatencyWifiLock = wifiMgr.createWifiLock(WifiManager.WIFI_MODE_FULL_LOW_LATENCY, BuildConfig.BRAND_NAME + " Low Latency Lock");
                    lowLatencyWifiLock.setReferenceCounted(false);
                    lowLatencyWifiLock.acquire();
                }
            }
        } catch (SecurityException e) {
            // Some Samsung Galaxy S10+/S10e devices throw a SecurityException from
            // WifiLock.acquire() even though we have android.permission.WAKE_LOCK in our manifest.
            e.printStackTrace();
        }

        appName = Game.this.getIntent().getStringExtra(EXTRA_APP_NAME);
        pcName = Game.this.getIntent().getStringExtra(EXTRA_PC_NAME);

        String host = Game.this.getIntent().getStringExtra(EXTRA_HOST);
        int port = Game.this.getIntent().getIntExtra(EXTRA_PORT, NvHTTP.DEFAULT_HTTP_PORT);
        int httpsPort = Game.this.getIntent().getIntExtra(EXTRA_HTTPS_PORT, 0); // 0 is treated as unknown
        int appId = Game.this.getIntent().getIntExtra(EXTRA_APP_ID, StreamConfiguration.INVALID_APP_ID);
        String uniqueId = Game.this.getIntent().getStringExtra(EXTRA_UNIQUEID);
        boolean appSupportsHdr = Game.this.getIntent().getBooleanExtra(EXTRA_APP_HDR, false);
        byte[] derCertData = Game.this.getIntent().getByteArrayExtra(EXTRA_SERVER_CERT);

        app = new NvApp(appName != null ? appName : "app", appId, appSupportsHdr);
        app.setAppUuid(getIntent().getStringExtra(EXTRA_APP_UUID));
        app.setMonitorResume(getIntent().getBooleanExtra(EXTRA_MONITOR_RESUME, false));
        app.setSmallStreamInputOnly(getIntent().getBooleanExtra(EXTRA_SMALL_STREAM_INPUT_ONLY, false));
        // Until the host answers, its last known capabilities decide whether this is a control tile.
        HostCapabilities cachedCaps = HostCapabilityCache.load(this, getIntent().getStringExtra(EXTRA_PC_UUID));
        app.setHostCapabilities(cachedCaps);
        boolean mediaStream = app.getRole() != NvApp.Role.INPUT_ONLY && !app.isControlAction();
        startInWindowShape(cachedCaps, mediaStream);

        X509Certificate serverCert = null;
        try {
            if (derCertData != null) {
                serverCert = (X509Certificate) CertificateFactory.getInstance("X.509")
                        .generateCertificate(new ByteArrayInputStream(derCertData));
            }
        } catch (CertificateException e) {
            e.printStackTrace();
        }

        if (appId == StreamConfiguration.INVALID_APP_ID && app.getAppUuid().isEmpty()) {
            finish();
            return;
        }

        // Initialize the MediaCodec helper before creating the decoder
        GlPreferences glPrefs = GlPreferences.readPreferences(this);
        MediaCodecHelper.initialize(this, glPrefs.glRenderer);

        // Check if the user has enabled HDR
        boolean willStreamHdr = false;
        if (mediaStream && prefConfig.enableHdr) {
            // HLG stands in for HDR10 unless the Vulkan renderer draws the picture on a PQ-only surface.
            HdrPolicy.Refusal refusal = HdrPolicy.displayRefusal(getVideoDisplay(), true,
                    cachedCaps == null ? 0 : cachedCaps.hdrFormats(),
                    MediaCodecDecoderRenderer.vulkanSurface(prefConfig));
            willStreamHdr = refusal == HdrPolicy.Refusal.NONE;
            if (!willStreamHdr) overlays.refuseHdr(refusal);
        }

        // Check if the user has enabled performance stats overlay
        if (mediaStream && prefConfig.enablePerfOverlay) {
            performanceOverlayScroll.setVisibility(View.VISIBLE);
        }

        decoderRenderer = new MediaCodecDecoderRenderer(
                this,
                prefConfig,
                new CrashListener() {
                    @Override
                    public void notifyCrash(Exception e) {
                        // A stream that is ending, or a TV that just lost its display, is not a decoder
                        // crash: counting it would turn reference frame invalidation off next time.
                        CrashAccounting.Verdict verdict = CrashAccounting.classify(streamEnding(),
                                isAndroidTv(), videoDisplayOff);
                        if (verdict != CrashAccounting.Verdict.COUNT) {
                            LimeLog.warning("Decoder failure not counted as a crash (" +
                                    (verdict == CrashAccounting.Verdict.DISPLAY_LOST ? "display lost" : "stream ending") +
                                    "): " + e);
                            return;
                        }
                        // The decoder, format and frame counts this crash is counted against.
                        LimeLog.severe("Decoder crash counted: " + e);

                        // The renderer reports a decoder it gave up on; the stream ends with its
                        // dialog, and the next one runs without reference frame invalidation.
                        synchronized (tombstonePrefs) {
                            if (reportedCrash) {
                                return;
                            }
                            reportedCrash = true;
                            CrashAccounting.recordGiveUp(tombstonePrefs, getIntent().getStringExtra(EXTRA_PC_UUID));
                        }
                    }
                },
                tombstonePrefs.getInt("CrashCount", 0),
                connMgr.isActiveNetworkMetered(),
                willStreamHdr,
                glPrefs.glRenderer,
                this, getVideoDisplay());
        decoderRenderer.setPackedFailureListener(this::onPackedChromaFailure);
        decoderRenderer.setStreamSizeListener(new MediaCodecDecoderRenderer.StreamSizeListener() {
            @Override
            public void onStreamSizeChanged(int width, int height) {
                runOnUiThread(() -> applyStreamSize(width, height));
            }

            @Override
            public void onDecodedSize(int decodedWidth, int decodedHeight, int shownWidth, int shownHeight) {
                runOnUiThread(() -> {
                    if (!isFinishing() && !isDestroyed()) {
                        streamView.setDecodedPadding(decodedWidth, decodedHeight, shownWidth, shownHeight);
                    }
                });
            }
        });

        // Don't stream HDR if the decoder can't support it
        if (willStreamHdr) {
            HdrPolicy.Refusal refusal = HdrPolicy.decoderRefusal(decoderRenderer.isHevcMain10Hdr10Supported(),
                    decoderRenderer.isAv1Main10Supported(), decoderRenderer.isPyroWaveSupported());
            if (refusal != HdrPolicy.Refusal.NONE) {
                willStreamHdr = false;
                overlays.refuseHdr(refusal);
            }
        }
        streamHdr = willStreamHdr;

        // PyroWave was chosen but this PC showed it has none; say once for the PC what is used instead.
        if (mediaStream && prefConfig.pyroWaveUnavailable) {
            messages.notice(getIntent().getStringExtra(EXTRA_PC_UUID), HostNotices.Notice.PYROWAVE_UNAVAILABLE,
                    getString(R.string.host_pyrowave_unavailable,
                            getResources().getStringArray(R.array.host_profile_codecs)[prefConfig.videoFormat.ordinal()]));
        } else if (prefConfig.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE) {
            HostNotices.reset(this, getIntent().getStringExtra(EXTRA_PC_UUID), HostNotices.Notice.PYROWAVE_UNAVAILABLE);
        }

        // Display a message to the user if HEVC was forced on but we still didn't find a decoder
        if (mediaStream && prefConfig.videoFormat == PreferenceConfiguration.FormatOption.FORCE_HEVC && !decoderRenderer.isHevcSupported()) {
            overlays.hostNotice(HostNotices.Notice.CODEC_UNAVAILABLE, R.string.stream_hevc_decoder_missing);
        }

        // Display a message to the user if AV1 was forced on but we still didn't find a decoder
        if (mediaStream && prefConfig.videoFormat == PreferenceConfiguration.FormatOption.FORCE_AV1 && !decoderRenderer.isAv1Supported()) {
            overlays.hostNotice(HostNotices.Notice.CODEC_UNAVAILABLE, R.string.stream_av1_decoder_missing);
        }

        // 10-bit SDR: the 10-bit formats without HDR. HDR, when granted, wins.
        boolean sdr10Hevc = mediaStream && decoderRenderer.isHevcMain10Supported();
        boolean sdr10Av1 = mediaStream && decoderRenderer.isAv1Main10SdrSupported();
        Sdr10Policy.Refusal sdr10Refusal = Sdr10Policy.refusal(mediaStream, prefConfig.enableSdr10Bit, willStreamHdr,
                sdr10Hevc, sdr10Av1, prefConfig.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE,
                prefConfig.experimentalPacked444);
        streamSdr10 = Sdr10Policy.formats(sdr10Refusal, sdr10Hevc, sdr10Av1) != 0;
        decoderRenderer.setSdr10Requested(streamSdr10);
        if (Sdr10Policy.worthSaying(sdr10Refusal)) {
            LimeLog.warning("10-bit SDR enabled but not used: " + sdr10Refusal);
            int reason = sdr10Refusal == Sdr10Policy.Refusal.PYROWAVE ? R.string.stream_sdr10_reason_pyrowave :
                    sdr10Refusal == Sdr10Policy.Refusal.PACKED_444 ? R.string.stream_sdr10_reason_packed :
                            R.string.stream_sdr10_reason_decoder;
            messages.notice(getIntent().getStringExtra(EXTRA_PC_UUID), HostNotices.Notice.SDR10_UNAVAILABLE,
                    getString(R.string.stream_sdr10_unavailable, getString(reason)));
        }
        // Native 4:4:4 over 10-bit SDR depends on the PC, so the connection decides it (and re-arms the notice).

        int supportedVideoFormats = mediaStream ? decoderRenderer.getSupportedVideoFormats(willStreamHdr, streamSdr10) : MoonBridge.VIDEO_FORMAT_H264;

        int gamepadMask = ControllerHandler.getAttachedControllerMask(this);
        if (!prefConfig.multiController) {
            // Always set gamepad 1 present for when multi-controller is
            // disabled for games that don't properly support detection
            // of gamepads removed and replugged at runtime.
            gamepadMask = 1;
        }
        if (prefConfig.onscreenController) {
            // If we're using OSC, always set at least gamepad 1.
            gamepadMask |= 1;
        }

        // Set to the optimal mode for streaming
        float displayRefreshRate = mediaStream ? prepareDisplayForRendering() :
                (desiredRefreshRate = getWindowManager().getDefaultDisplay().getRefreshRate());
        LimeLog.info("Display refresh rate: "+displayRefreshRate);

        // If the user requested frame pacing using a capped FPS, we will need to change our
        // desired FPS setting here in accordance with the active display refresh rate.
        int roundedRefreshRate = Math.round(displayRefreshRate);
        int chosenFrameRate = prefConfig.fps;
        if (prefConfig.framePacing == PreferenceConfiguration.FRAME_PACING_CAP_FPS) {
            if (prefConfig.fps >= roundedRefreshRate) {
                if (prefConfig.fps > roundedRefreshRate + 3) {
                    // Use frame drops when rendering above the screen frame rate
                    prefConfig.framePacing = PreferenceConfiguration.FRAME_PACING_BALANCED;
                    LimeLog.info("Using drop mode for FPS > Hz");
                } else if (roundedRefreshRate <= 49) {
                    // Let's avoid clearly bogus refresh rates and fall back to legacy rendering
                    prefConfig.framePacing = PreferenceConfiguration.FRAME_PACING_BALANCED;
                    LimeLog.info("Bogus refresh rate: " + roundedRefreshRate);
                }
                else {
                    chosenFrameRate = roundedRefreshRate - 1;
                    LimeLog.info("Adjusting FPS target for screen to " + chosenFrameRate);
                }
            }
        }

        // The capped-FPS policy above can choose Balanced too. Apply the external
        // output fallback after that decision so it cannot reintroduce phone-vsync pacing.
        if (mediaStream && decoderRenderer.applyOutputPacingFallback() &&
                prefConfig.videoFormat != PreferenceConfiguration.FormatOption.FORCE_PYROWAVE) {
            // The session's own setup, not a choice to make: the overlay's status line says it.
            messages.note(getString(R.string.desktop_pacing_fallback));
        }

        streamFps = chosenFrameRate;
        streamPanelHz = panelMaxRefreshRate;

        // Input-only without the host's own mode: the smallest ordinary stream, which leaves the
        // PC's display mode and its speakers alone.
        boolean smallStream = app.isSmallStreamInputOnly();
        StreamConfiguration config = new StreamConfiguration.Builder()
                .setResolution(smallStream ? InputOnlyPolicy.SMALL_WIDTH : prefConfig.width,
                        smallStream ? InputOnlyPolicy.SMALL_HEIGHT : prefConfig.height)
                .setLaunchRefreshRate(smallStream ? InputOnlyPolicy.SMALL_FPS : prefConfig.fps)
                .setLaunchRefreshRateX100(smallStream ? 0 : prefConfig.launchRefreshRateX100)
                .setRefreshRate(smallStream ? InputOnlyPolicy.SMALL_FPS : chosenFrameRate)
                .setApp(app)
                .setResumeOnly(isResumeOnlyStream())
                .setAllowIdleSessionEnd(!getIntent().getBooleanExtra(EXTRA_RECOVERY_MEDIA_FAILURE, false))
                .setBitrate(smallStream ? InputOnlyPolicy.SMALL_BITRATE_KBPS : prefConfig.bitrate)
                .setProbeFirstBitrate(!smallStream && prefConfig.probesFirstBitrate())
                .setAutomaticBitrate(!smallStream && prefConfig.automaticBitrate)
                .setBitrateLimit(StreamRateChanges.clientLimitKbps(prefConfig.layers == null ? null : prefConfig.layers.network,
                        prefConfig.mobileBitrateLimit))
                .setFecChoice(prefConfig.fecChoice)
                .setRateChange(!smallStream && prefConfig.rustEngine)
                .setEnableSops(!smallStream && prefConfig.enableSops)
                .enableLocalAudioPlayback(smallStream || prefConfig.playHostAudio)
                .setMaxPacketSize(1392)
                .setRemoteConfiguration(StreamConfiguration.STREAM_CFG_AUTO) // NvConnection will perform LAN and VPN detection
                .setSupportedVideoFormats(supportedVideoFormats)
                .setSdr10Bit(streamSdr10)
                .setPacked444(prefConfig.experimentalPacked444)
                .setAttachedGamepadMask(gamepadMask)
                .setClientRefreshRateX100(Math.round(desiredRefreshRate * 100))
                .setVirtualDisplay(!smallStream && prefConfig.virtualDisplay, prefConfig.virtualDisplayScale)
                .setVirtualDisplayDpi(prefConfig.virtualDisplayDpi)
                .setVrr(!smallStream && prefConfig.vrr)
                .setClipboardSync(prefConfig.clipboardSync && prefConfig.rustEngine)
                .setHostDisplay(prefConfig.hostDisplay)
                .setAudioConfiguration(smallStream ? MoonBridge.AUDIO_CONFIGURATION_STEREO : prefConfig.audioConfiguration)
                .setColorSpace(decoderRenderer.getPreferredColorSpace())
                .setColorRange(decoderRenderer.getPreferredColorRange())
                .setPersistGamepadsAfterDisconnect(!prefConfig.multiController)
                .setDisplayLuminance(smallStream ? null : launchLuminance(getVideoDisplay()))
                .build();

        // Initialize the connection
        conn = new NvConnection(getApplicationContext().createConfigurationContext(getResources().getConfiguration()),
                new ComputerDetails.AddressTuple(host, port),
                httpsPort, uniqueId, config,
                PlatformBinding.getCryptoProvider(this), serverCert);
        restoreMicrophoneMute();
        conn.setMicrophoneNoticeListener(() -> runOnUiThread(this::showMicrophoneNotice));
        controllerHandler = new ControllerHandler(this, conn, this, prefConfig);
        conn.setControllerPreparation(controllerHandler::prepareControllersForLaunch);
        updateInputPollingRate();
        ((android.hardware.display.DisplayManager) getSystemService(DISPLAY_SERVICE))
                .registerDisplayListener(inputDisplayListener, bitrateHandler);
        keyboardTranslator = new KeyboardTranslator();

        InputManager inputManager = (InputManager) getSystemService(Context.INPUT_SERVICE);
        inputManager.registerInputDeviceListener(keyboardTranslator, null);
        inputManager.registerInputDeviceListener(pcKeyboardDeviceListener, null);

        touchInput.initializeTouchContexts();

        desktopGestures = new com.limelight.ui.DesktopGestures(this, streamView,
                touchInput::cancelHostTouchInput, touchInput::clickRightMouseButton);
        trackpadPointer.setViewport(desktopGestures.viewport());
        if (app.getRole() != NvApp.Role.REMOTE_MONITOR) {
            String profile = com.limelight.binding.input.TouchLayout.profileKey(getIntent().getStringExtra(EXTRA_PC_UUID),
                    app.getAppUuid(), app.getAppId());
            gameTouchControls = new com.limelight.ui.GameTouchControls(this, profile,
                    new com.limelight.binding.input.HeldInputState.Sink() {
                        @Override public void key(int key, boolean down) { keyboardInput.sendCustomKey(key, down); }
                        @Override public void mouse(int button, boolean down) { touchInput.sendCustomMouse(button, down); }
                        @Override public void gamepad(int buttons, short leftX, short leftY, short rightX, short rightY,
                                                      byte leftTrigger, byte rightTrigger) {
                            if (controllerHandler != null) controllerHandler.reportLayoutGamepadState(buttons,
                                    leftX, leftY, rightX, rightY, leftTrigger, rightTrigger);
                        }
                    }, () -> foreground && connected && grabbedInput && streamMenu == null && conn.canSendInput(),
                    this::showStreamDialog);
            gameTouchControls.setStreamTouchListener(touchInput::onTouchControlsPassthrough);
            gameTouchControls.setGestureCancelListener(touchInput::cancelTouchInput);
            ((FrameLayout)streamView.getParent()).addView(gameTouchControls,
                    new FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT));
        }

        if (prefConfig.onscreenController && app.getRole() != NvApp.Role.REMOTE_MONITOR) {
            showOnscreenController();
        }
        if (prefConfig.enableMic && app.getRole() != NvApp.Role.REMOTE_MONITOR) {
            micButton = new com.limelight.ui.MicButton(this, new com.limelight.ui.MicButton.Host() {
                @Override public boolean offered() { return conn != null && conn.getMicrophoneStatus().canMute(); }
                @Override public boolean muted() { return conn != null && conn.isMicrophoneMuted(); }
                @Override public void setMuted(boolean muted) { if (conn != null) conn.setMicrophoneMuted(muted); }
                @Override public float topRowBottom() {
                    return modernController != null && prefConfig.onscreenController ? modernController.topRowBottom() : Float.NaN;
                }
            });
            ((FrameLayout) streamView.getParent()).addView(micButton, micButton.layoutParams());
            touchInput.updateMicButton();
        }

        if ((prefConfig.usbDriver || prefConfig.usbDualSense || prefConfig.usbSwitchPro) && app.getRole() != NvApp.Role.REMOTE_MONITOR) {
            // Start the USB driver
            usbDriverServiceBound = bindService(new Intent(this, UsbDriverService.class),
                    usbDriverServiceConnection, Service.BIND_AUTO_CREATE);
        }

        if (mediaStream && !decoderRenderer.isAvcSupported()) {
            if (spinner != null) {
                spinner.dismiss();
                spinner = null;
            }

            // If we can't find an AVC decoder, we can't proceed
            Dialog.displayDialog(this, getResources().getString(R.string.conn_error_title),
                    getString(R.string.stream_h264_decoder_missing), true);
            return;
        }

        // The connection will be started when the surface gets created
        if (prefConfig.useTextureView) {
            renderView.getTextureView().setSurfaceTextureListener(this);
            if (renderView.getTextureView().isAvailable()) {
                onSurfaceTextureAvailable(renderView.getTextureView().getSurfaceTexture(), renderView.getWidth(), renderView.getHeight());
            }
        } else {
            renderView.getHolder().addCallback(this);
        }
    }

    Display getVideoDisplay() {
        // The intent describes the next output while a reconnect is in progress. Keep
        // reporting capabilities for the surface that this activity actually renders.
        if (renderView != null) {
            return externalPresentation == null ? getWindowManager().getDefaultDisplay() :
                    externalPresentation.getDisplay();
        }
        int requested = getIntent().getIntExtra(EXTRA_OUTPUT_DISPLAY, Display.DEFAULT_DISPLAY);
        if (requested != Display.DEFAULT_DISPLAY) {
            for (Display display : ((android.hardware.display.DisplayManager)getSystemService(DISPLAY_SERVICE))
                    .getDisplays(android.hardware.display.DisplayManager.DISPLAY_CATEGORY_PRESENTATION)) {
                if (display.getDisplayId() == requested && display.isValid()) return display;
            }
        }
        return getWindowManager().getDefaultDisplay();
    }

    private android.view.Window getVideoWindow() {
        return externalPresentation == null ? getWindow() : externalPresentation.getWindow();
    }

    private void initializeVideoOutput() {
        Display display = getVideoDisplay();
        renderView = streamView;
        if (display.getDisplayId() != getWindowManager().getDefaultDisplay().getDisplayId()) {
            com.limelight.ui.StreamPresentation presentation = null;
            try {
                presentation = new com.limelight.ui.StreamPresentation(this, display, prefConfig.useTextureView);
                externalPresentation = presentation;
                renderView = presentation.videoView();
                presentation.setOnDismissListener(ignored -> {
                    if (!isFinishing() && !isDestroyed() && !outputSwitchPending) returnVideoToDevice();
                });
                presentation.show();
                prefConfig.touchscreenTrackpad = true;
                // The phone is the touchpad; a pointer drawn on it would not match the external screen.
                if (prefConfig.touchMode != TouchMode.OFF) prefConfig.touchMode = TouchMode.TRACKPAD;
                nativeTouchEnabled = false;
                TextView hint = findViewById(R.id.inputOnlyStatus);
                hint.setText(R.string.desktop_touchpad_help); hint.setVisibility(View.VISIBLE);
                // Device orientation and window changes must not resize the external video.
                if (nativeResolutionStream) {
                    Point size = new Point(); display.getRealSize(size);
                    if (size.x > 0 && size.y > 0) { prefConfig.width = size.x; prefConfig.height = size.y; }
                }
                nativeResolutionStream = false;
                return;
            } catch (WindowManager.InvalidDisplayException | SecurityException | IllegalArgumentException e) {
                if (presentation != null) {
                    presentation.setOnDismissListener(null);
                    presentation.dismiss();
                }
                externalPresentation = null; renderView = streamView;
                getIntent().removeExtra(EXTRA_OUTPUT_DISPLAY);
                messages.alert(R.string.desktop_display_unavailable, Toast.LENGTH_LONG);
            }
        }
        getIntent().removeExtra(EXTRA_OUTPUT_DISPLAY);
        // A hot-unplug can happen between reading preferences and creating the
        // presentation. Do not retain the missing monitor's VRR/FPS launch values.
        panelMaxRefreshRate = com.limelight.binding.video.DisplayFrameRatePolicy.maxRefreshRate(getVideoDisplay());
        if (prefConfig.vrr) {
            prefConfig.fps = com.limelight.binding.video.DisplayFrameRatePolicy.streamFrameRate(panelMaxRefreshRate);
            prefConfig.launchRefreshRateX100 = Math.max(100, Math.min(100000, Math.round(panelMaxRefreshRate * 100)));
        }
        fpsFollowsDisplay = prefConfig.vrr || prefConfig.fps ==
                com.limelight.binding.video.DisplayFrameRatePolicy.streamFrameRate(panelMaxRefreshRate);
        streamView.initializeSurface(prefConfig.useTextureView);
    }

    private void returnVideoToDevice() {
        if (outputSwitchPending || isFinishing() || isDestroyed()) return;
        outputSwitchPending = true;
        getIntent().removeExtra(EXTRA_OUTPUT_DISPLAY);
        if (foreground && conn != null) reconnectStream();
        else {
            rememberStreamForResume(true);
            if (stoppedStreamResumeIntent != null) {
                stoppedStreamResumeIntent.removeExtra(EXTRA_OUTPUT_DISPLAY);
                displayedFailureDialog = true;
                stopConnection();
            } else finish();
        }
    }

    private void toggleDesktopZoom() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N && !prefConfig.useTextureView) {
            showStreamDialog(new NavigationDialogBuilder(this).setMessage(R.string.desktop_zoom_surface_help)
                    .setPositiveButton(android.R.string.ok, null).create());
            return;
        }
        desktopGestures.setEnabled(!desktopGestures.enabled());
        if (desktopGestures.enabled()) messages.alert(R.string.desktop_zoom_help, Toast.LENGTH_LONG);
    }

    private boolean hasPresentationDisplays() {
        return ((android.hardware.display.DisplayManager) getSystemService(DISPLAY_SERVICE))
                .getDisplays(android.hardware.display.DisplayManager.DISPLAY_CATEGORY_PRESENTATION).length > 0;
    }

    // The view changes its scaling at once. An upscaler or PyroWave sets its own output rectangle
    // when the stream starts, so for those the choice waits for the next stream.
    private boolean canScaleLive() {
        return externalPresentation == null && decoderRenderer != null &&
                prefConfig.upscalingMode == UpscalingPolicy.Mode.OFF &&
                (decoderRenderer.getActiveVideoFormat() & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) == 0;
    }

    private void showVideoScalingDialog() {
        if (!canScaleLive()) return;
        showStreamDialog(new NavigationDialogBuilder(this).setTitle(R.string.video_scaling_title)
                .setSingleChoiceItems(R.array.video_scaling_modes, prefConfig.videoScaling, (dialog, which) -> {
                    dialog.dismiss();
                    setVideoScaling(which);
                }).setNegativeButton(android.R.string.cancel, null).create());
    }

    private void setVideoScaling(int mode) {
        if (mode == prefConfig.videoScaling || !canScaleLive()) return;
        prefConfig.setVideoScaling(mode);
        SharedPreferences.Editor editor = PreferenceManager.getDefaultSharedPreferences(this).edit();
        VideoScaling.save(editor, mode);
        editor.apply();
        applyVideoScaling();
        // Fill and Stretch span the cutout too; the safe area follows.
        configureStreamWindow();
        updatePipAutoEnter();
    }

    /** Sizes the video view for the scaling mode; picture-in-picture always shows the whole stream. */
    private void applyVideoScaling() {
        if (renderView == null || prefConfig == null) return;
        int mode = prefConfig.videoScaling;
        if (renderView == streamView && Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && isInPictureInPictureMode()) {
            mode = VideoScaling.forPictureInPicture(mode);
        }
        renderView.setVideoScaling(mode, prefConfig.videoAlignment, prefConfig.width, prefConfig.height);
    }

    private boolean canEnterPictureInPicture() {
        return Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && prefConfig.enablePip && externalPresentation == null &&
                app.getRole() != NvApp.Role.INPUT_ONLY &&
                getPackageManager().hasSystemFeature(PackageManager.FEATURE_PICTURE_IN_PICTURE);
    }

    private void enterPictureInPictureNow() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O || !canEnterPictureInPicture() || !connected) return;
        try {
            enterPictureInPictureMode(getPictureInPictureParams(false));
        } catch (IllegalStateException | IllegalArgumentException e) {
            LimeLog.warning("Picture-in-picture unavailable");
        }
    }

    private void showVideoOutputDialog() {
        Display[] displays = ((android.hardware.display.DisplayManager)getSystemService(DISPLAY_SERVICE))
                .getDisplays(android.hardware.display.DisplayManager.DISPLAY_CATEGORY_PRESENTATION);
        String[] labels = new String[displays.length + 1]; labels[0] = getString(R.string.desktop_output_device);
        for (int i = 0; i < displays.length; i++) labels[i + 1] = displays[i].getName();
        showStreamDialog(new NavigationDialogBuilder(this).setTitle(R.string.desktop_output)
                .setItems(labels, (dialog, which) -> {
                    int id = which == 0 ? Display.DEFAULT_DISPLAY : displays[which - 1].getDisplayId();
                    if (id == getIntent().getIntExtra(EXTRA_OUTPUT_DISPLAY, Display.DEFAULT_DISPLAY)) return;
                    streamMenuAction = () -> {
                        outputSwitchPending = true;
                        getIntent().putExtra(EXTRA_OUTPUT_DISPLAY, id);
                        reconnectStream();
                    };
                }).setNegativeButton(android.R.string.cancel, null).create());
        if (displays.length == 0) messages.alert(R.string.desktop_display_unavailable, Toast.LENGTH_LONG);
    }

    private void setPreferredOrientationForCurrentDisplay() {
        boolean freeWindow = getIntent().getIntExtra(EXTRA_OUTPUT_DISPLAY, Display.DEFAULT_DISPLAY) != Display.DEFAULT_DISPLAY ||
                (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N && isInMultiWindowMode());
        boolean largeScreen = getResources().getConfiguration().smallestScreenWidthDp >= 600 ||
                getPackageManager().hasSystemFeature("android.hardware.type.pc");
        boolean squarish = !freeWindow && !largeScreen &&
                PreferenceConfiguration.isSquarishScreen(getWindowManager().getDefaultDisplay());
        PreferenceConfiguration config = prefConfig != null ? prefConfig : new PreferenceConfiguration();
        setRequestedOrientation(StreamOrientation.requested(streamOrientation, freeWindow, largeScreen, squarish,
                config.onscreenController, nativeResolutionStream, config.width, config.height));
    }

    // A host that draws the stream on its virtual display starts it in the shape the Orientation
    // setting gives the window; any other host keeps the size and the video is scaled into it.
    private void startInWindowShape(HostCapabilities caps, boolean mediaStream) {
        HostDisplayChoice display = prefConfig.hostDisplay == null || !caps.has(HostCapabilities.Feature.HOST_DISPLAY_MODES) ?
                HostDisplayChoice.HOST_DEFAULT : prefConfig.hostDisplay.forRole(app.getRole());
        hostShapesStream = mediaStream && externalPresentation == null && app.getRole() == NvApp.Role.STREAM &&
                caps.has(HostCapabilities.Feature.VIRTUAL_DISPLAY) && display.requestsVirtualDisplay(prefConfig.virtualDisplay);
        boolean windowPortrait = getResources().getConfiguration().orientation == Configuration.ORIENTATION_PORTRAIT;
        int[] size = StreamOrientation.launchSize(streamOrientation, hostShapesStream, windowPortrait,
                prefConfig.width, prefConfig.height);
        if (size[0] != prefConfig.width || size[1] != prefConfig.height) {
            LimeLog.info("Orientation " + StreamOrientation.value(streamOrientation) + ": stream request turned to " +
                    size[0] + "x" + size[1]);
            prefConfig.width = size[0];
            prefConfig.height = size[1];
        }
        launchWidth = prefConfig.width;
        launchHeight = prefConfig.height;
        launchPanel = PreferenceConfiguration.nativeResolution(this);
    }

    static boolean isValidLaunch(String host, int port, String uniqueId, int appId, String appUuid) {
        return host != null && !host.trim().isEmpty() && port > 0 && port <= 65535
                && uniqueId != null && !uniqueId.isEmpty()
                && (appId != StreamConfiguration.INVALID_APP_ID || (appUuid != null && !appUuid.isEmpty()));
    }

    @Override
    public void onConfigurationChanged(Configuration newConfig) {
        super.onConfigurationChanged(newConfig);
        configureStreamWindow();
        if (controllerHandler == null || isFinishing()) {
            return;
        }

        // Set requested orientation for possible new screen size
        setPreferredOrientationForCurrentDisplay();
        // Attaching a hardware keyboard or switching the IME's language changes the layout.
        keyboardInput.reportKeyboardLayout();

        // A fold, rotation or window change may change the native size
        boolean turned = newConfig.orientation != lastWindowOrientation;
        lastWindowOrientation = newConfig.orientation;
        if (turned) noteRotationWithoutReconfigure();
        scheduleReconfigureCheck(StreamOrientation.settleMs(streamOrientation, hostShapesStream, turned,
                StreamReconfigureRequester.SETTLE_MS));

        if (virtualController != null && prefConfig.onscreenController) {
            // Refresh layout of OSC for possible new screen size
            virtualController.refreshLayout();
        }
        // The modern controller and the microphone button follow their frame's size by themselves.

        applyVideoScaling();

        // Hide on-screen overlays in PiP mode
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            boolean inPictureInPicture = isInPictureInPictureMode();
            boolean leftPictureInPicture = wasInPictureInPicture && !inPictureInPicture;
            wasInPictureInPicture = inPictureInPicture;
            if (inPictureInPicture) {
                isHidingOverlays = true;
                if (gameTouchControls != null) gameTouchControls.hideForPip(true);

                if (virtualController != null) {
                    virtualController.hide();
                }
                if (modernController != null) {
                    modernController.finishEditing(false);
                    modernController.hide();
                }
                touchInput.updateMicButton();
                if (keyboardOverlay != null) {
                    keyboardInput.hideKeyboardOverlay();
                }
                keyboardInput.hidePcKeyboard();

                performanceOverlayScroll.setVisibility(View.GONE);
                notificationOverlayView.setVisibility(View.GONE);
                overlays.dismissStreamMenuHint(false);

                // Disable sensors while in PiP mode
                controllerHandler.disableSensors();

                if (trackpadPointer != null) {
                    trackpadPointer.stopMomentum();
                    trackpadPointer.setCursorShown(false);
                }

                // Update GameManager state to indicate we're in PiP (still gaming, but interruptible)
                UiHelper.notifyStreamEnteringPiP(this);
            }
            // Rotation, folding, a keyboard or dark mode also change the configuration; only a real
            // way out of PiP restores what it hid.
            else if (leftPictureInPicture) {
                isHidingOverlays = false;
                boolean canSendInput = conn != null && conn.canSendInput();
                if (trackpadPointer != null) trackpadPointer.setCursorShown(true);
                // A session without input permission keeps its controls hidden.
                if (gameTouchControls != null && canSendInput) gameTouchControls.hideForPip(false);

                // Restore overlays to previous state when leaving PiP

                if (virtualController != null && prefConfig.onscreenController && canSendInput) {
                    virtualController.show();
                }
                if (modernController != null && prefConfig.onscreenController && canSendInput) {
                    modernController.show();
                }
                touchInput.updateMicButton();

                if (prefConfig.enablePerfOverlay && app.getRole() != NvApp.Role.INPUT_ONLY) {
                    performanceOverlayScroll.setVisibility(View.VISIBLE);
                }

                notificationOverlayView.setVisibility(requestedNotificationOverlayVisibility);

                // Enable sensors again after exiting PiP
                controllerHandler.enableSensors();

                // Update GameManager state to indicate we're out of PiP (gaming, non-interruptible)
                UiHelper.notifyStreamExitingPiP(this);
            }
        }
        overlays.updateHardwareMetricsPolling();
    }

    @TargetApi(Build.VERSION_CODES.O)
    private PictureInPictureParams getPictureInPictureParams(boolean autoEnter) {
        int[] ratio = PictureInPicturePolicy.aspectRatio(prefConfig.width, prefConfig.height);
        Rect source = new Rect();
        streamView.getGlobalVisibleRect(source);
        PictureInPictureParams.Builder builder =
                new PictureInPictureParams.Builder()
                        .setAspectRatio(new Rational(ratio[0], ratio[1]))
                        .setSourceRectHint(source);

        if (pipDisconnectIntent == null) {
            String action = getPackageName() + ".PIP_DISCONNECT." + java.util.UUID.randomUUID();
            androidx.core.content.ContextCompat.registerReceiver(this, pipReceiver, new IntentFilter(action),
                    androidx.core.content.ContextCompat.RECEIVER_NOT_EXPORTED);
            pipDisconnectIntent = PendingIntent.getBroadcast(this, 0,
                    new Intent(action).setPackage(getPackageName()), PendingIntent.FLAG_IMMUTABLE);
        }
        RemoteAction disconnect = new RemoteAction(Icon.createWithResource(this, R.drawable.ic_close),
                getString(R.string.stream_disconnect), getString(R.string.pip_disconnect_description), pipDisconnectIntent);
        disconnect.setEnabled(connected && app.getRole() != NvApp.Role.INPUT_ONLY);
        builder.setActions(java.util.Collections.singletonList(disconnect));

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            builder.setAutoEnterEnabled(autoEnter);
            builder.setSeamlessResizeEnabled(true);
        }

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            if (appName != null) {
                builder.setTitle(appName);
                if (pcName != null) {
                    builder.setSubtitle(pcName);
                }
            }
            else if (pcName != null) {
                builder.setTitle(pcName);
            }
        }

        return builder.build();
    }

    private void updatePipAutoEnter() {
        if (prefConfig == null || !prefConfig.enablePip || isFinishing() || isDestroyed() ||
                !getPackageManager().hasSystemFeature(PackageManager.FEATURE_PICTURE_IN_PICTURE)) {
            return;
        }

        boolean autoEnter = externalPresentation == null && PictureInPicturePolicy.shouldAutoEnter(connected, app.getRole(), suppressPipRefCount,
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.N && isInMultiWindowMode());

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            try {
                setPictureInPictureParams(getPictureInPictureParams(autoEnter));
            } catch (IllegalStateException | IllegalArgumentException e) {
                LimeLog.warning("Picture-in-picture unavailable");
            }
        }
        else {
            autoEnterPip = autoEnter;
        }
    }

    public void setMetaKeyCaptureState(boolean enabled) {
        keyboardInput.setMetaKeyCaptureState(enabled);
    }

    @Override
    public void onUserLeaveHint() {
        super.onUserLeaveHint();

        // PiP is only supported on Oreo and later, and we don't need to manually enter PiP on
        // Android S and later. On Android R, we will use onPictureInPictureRequested() instead.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            if (autoEnterPip) {
                try {
                    // This has thrown all sorts of weird exceptions on Samsung devices
                    // running Oreo. Just eat them and close gracefully on leave, rather
                    // than crashing.
                    enterPictureInPictureMode(getPictureInPictureParams(false));
                } catch (Exception e) {
                    e.printStackTrace();
                }
            }
        }
    }

    @Override
    @TargetApi(Build.VERSION_CODES.R)
    public boolean onPictureInPictureRequested() {
        // Enter PiP when requested unless we're on Android 12 which supports auto-enter.
        if (autoEnterPip && Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            try {
                enterPictureInPictureMode(getPictureInPictureParams(false));
            } catch (IllegalStateException | IllegalArgumentException e) {
                LimeLog.warning("Picture-in-picture unavailable");
            }
        }
        return true;
    }

    @Override
    public boolean dispatchTouchEvent(MotionEvent event) {
        // A direct press can reach a split-screen window before its focus notification.
        // Only callbacks inside this dispatch may use that press as evidence of interaction.
        boolean previous = dispatchingDesktopTouch;
        dispatchingDesktopTouch = event.isFromSource(InputDevice.SOURCE_CLASS_POINTER) &&
                event.getActionMasked() != MotionEvent.ACTION_CANCEL &&
                (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
                        (event.getFlags() & MotionEvent.FLAG_CANCELED) == 0);
        try {
            return super.dispatchTouchEvent(event);
        } finally {
            dispatchingDesktopTouch = previous;
        }
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        desktopWindowFocused = hasFocus;

        if (!hasFocus) {
            touchInput.cancelTouchInput();
            keyboardInput.releaseKeyboardInput();
            touchInput.releaseMouseButtons();
            if (virtualController != null) {
                virtualController.releaseInput();
            }
            if (modernController != null) {
                modernController.releaseInput();
            }
        }
        if (controllerHandler != null) {
            controllerHandler.setInputEnabled(hasFocus && grabbedInput);
        }

        // We can't guarantee the state of modifiers keys which may have
        // lifted while focus was not on us. Clear the modifier state.
        this.modifierFlags = 0;

        // With Android native pointer capture, capture is lost when focus is lost,
        // so it must be requested again when focus is regained.
        if (inputCaptureProvider != null) {
            inputCaptureProvider.onWindowFocusChanged(hasFocus);
        }
        // A copy made in another app goes once the stream is back in front.
        if (clipboardSync != null) {
            clipboardSync.focusChanged(hasFocus);
        }
        if (hasFocus) {
            // Let Android finish restoring IME focus before running a menu action.
            getWindow().getDecorView().post(runWindowFocusActions);
        }
    }

    private boolean isRefreshRateEqualMatch(float refreshRate) {
        return refreshRate >= prefConfig.fps &&
                refreshRate <= prefConfig.fps + 3;
    }

    private boolean isRefreshRateGoodMatch(float refreshRate) {
        return refreshRate >= prefConfig.fps &&
                Math.round(refreshRate) % prefConfig.fps <= 3;
    }

    private boolean shouldIgnoreInsetsForResolution(int width, int height) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            Display display = getWindowManager().getDefaultDisplay();
            for (Display.Mode candidate : display.getSupportedModes()) {
                // Ignore insets if this is an exact match for the display resolution
                if ((width == candidate.getPhysicalWidth() && height == candidate.getPhysicalHeight()) ||
                        (height == candidate.getPhysicalWidth() && width == candidate.getPhysicalHeight())) {
                    return true;
                }
            }
        }

        // The panel as the window manager reports it right now, for devices whose mode list
        // describes a different (or scaled) size than the screen the stream fills.
        android.graphics.Point real = new android.graphics.Point();
        getWindowManager().getDefaultDisplay().getRealSize(real);
        if ((width == real.x && height == real.y) || (width == real.y && height == real.x)) {
            return true;
        }
        // A stream in the panel's own shape at another size (a scaled native size, or a PC profile's
        // "match screen" size) is drawn for the whole panel too: keeping the cutout clear would only
        // shrink it and leave a strip. The cutout-safe Native entry is narrower by the cutout and
        // keeps its strip.
        return PreferenceConfiguration.hasPanelShape(this, width, height, PANEL_SHAPE_TOLERANCE);
    }

    // Rounding a scaled panel size to even pixels moves its shape by well under this; leaving out a
    // camera cutout (54 px of a Fold's 2184, 80 px of a phone's 2400) moves it by 2-3%.
    private static final double PANEL_SHAPE_TOLERANCE = 0.005;

    @Override
    public boolean isWindowedStream() {
        return Build.VERSION.SDK_INT >= Build.VERSION_CODES.N && isInMultiWindowMode() && !windowCoversDisplay();
    }

    /** A free-form or pop-up window maximised to the whole display reports multi-window mode too. */
    protected boolean windowCoversDisplay() {
        androidx.window.layout.WindowMetricsCalculator calculator =
                androidx.window.layout.WindowMetricsCalculator.getOrCreate();
        return calculator.computeCurrentWindowMetrics(this).getBounds()
                .equals(calculator.computeMaximumWindowMetrics(this).getBounds());
    }

    /** Fullscreen native/fill video uses the complete panel; fixed presets retain a cutout-safe area. */
    @Override
    public boolean usesFullDisplayForVideo() {
        // nativeResolutionStream also includes custom sizes and the explicit cutout-safe
        // Native entry. Only the user's fill/follow choice or an exact panel size spans a notch.
        return prefConfig != null && (VideoScaling.coversArea(prefConfig.videoScaling) ||
                prefConfig.resolutionFollowsScreen || shouldIgnoreInsetsForResolution(prefConfig.width, prefConfig.height));
    }

    private void configureStreamWindow() {
        // Let AdaptiveLayout own the safe area. Combining framework cutout avoidance with
        // root padding cropped the video twice on older Android versions, and DEFAULT is
        // treated as ALWAYS on Android 15+ regardless of the raw layout attribute.
        WindowCompat.setDecorFitsSystemWindows(getWindow(), false);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            WindowManager.LayoutParams attributes = getWindow().getAttributes();
            attributes.layoutInDisplayCutoutMode = Build.VERSION.SDK_INT >= Build.VERSION_CODES.R ?
                    WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS :
                    WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
            getWindow().setAttributes(attributes);
        }
        View root = findViewById(android.R.id.content);
        if (root != null) ViewCompat.requestApplyInsets(root);
    }

    private boolean mayReduceRefreshRate() {
        return prefConfig.framePacing == PreferenceConfiguration.FRAME_PACING_CAP_FPS ||
                prefConfig.framePacing == PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS ||
                prefConfig.reduceRefreshRate;
    }

    private float prepareDisplayForRendering() {
        Display display = getVideoDisplay();
        WindowManager.LayoutParams windowLayoutParams = getVideoWindow().getAttributes();
        float displayRefreshRate;

        boolean arrSupported = Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA && display.hasArrSupport();
        useArr = com.limelight.binding.video.DisplayFrameRatePolicy.useAdaptiveRefreshRate(
                Build.VERSION.SDK_INT, prefConfig.vrr, arrSupported);
        logDisplayRefreshPolicy(display, arrSupported);
        if (prefConfig.vrr) {
            com.limelight.binding.video.DisplayFrameRatePolicy.ArrState arr =
                    com.limelight.binding.video.DisplayFrameRatePolicy.arrState(Build.VERSION.SDK_INT, true, arrSupported);
            LimeLog.info(useArr ? "VRR: ARR (Android automatic policy)" :
                    "VRR: max Hz (ARR unavailable: " + arr + ")");
            // Asked for and not granted: the status line says why, in place of a silent fallback.
            if (arr == com.limelight.binding.video.DisplayFrameRatePolicy.ArrState.ANDROID_TOO_OLD) {
                messages.note(getString(R.string.stream_vrr_old_android));
            } else if (arr == com.limelight.binding.video.DisplayFrameRatePolicy.ArrState.PANEL_WITHOUT_ARR) {
                messages.note(getString(R.string.stream_vrr_no_arr));
            }
        }

        if (useArr && Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA) {
            // Leave refresh-rate selection and touch boost to Android's automatic ARR policy.
            windowLayoutParams.preferredDisplayModeId = 0;
            windowLayoutParams.preferredRefreshRate = 0;
            getVideoWindow().setAttributes(windowLayoutParams);
            displayRefreshRate = panelMaxRefreshRate;
        }
        // On M, we can explicitly set the optimal display mode
        else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            Display.Mode bestMode = display.getMode();
            boolean isNativeResolutionStream = PreferenceConfiguration.isNativeResolution(prefConfig.width, prefConfig.height);
            boolean refreshRateIsGood = isRefreshRateGoodMatch(bestMode.getRefreshRate());
            boolean refreshRateIsEqual = isRefreshRateEqualMatch(bestMode.getRefreshRate());

            LimeLog.info("Current display mode: "+bestMode.getPhysicalWidth()+"x"+
                    bestMode.getPhysicalHeight()+"x"+bestMode.getRefreshRate());

            for (Display.Mode candidate : display.getSupportedModes()) {
                boolean refreshRateReduced = candidate.getRefreshRate() < bestMode.getRefreshRate();
                boolean resolutionReduced = candidate.getPhysicalWidth() < bestMode.getPhysicalWidth() ||
                        candidate.getPhysicalHeight() < bestMode.getPhysicalHeight();
                boolean resolutionFitsStream = candidate.getPhysicalWidth() >= prefConfig.width &&
                        candidate.getPhysicalHeight() >= prefConfig.height;

                LimeLog.info("Examining display mode: "+candidate.getPhysicalWidth()+"x"+
                        candidate.getPhysicalHeight()+"x"+candidate.getRefreshRate());

                if (prefConfig.vrr) {
                    if (candidate.getRefreshRate() > bestMode.getRefreshRate()) bestMode = candidate;
                    continue;
                }

                if (candidate.getPhysicalWidth() > 4096 && prefConfig.width <= 4096) {
                    // Avoid resolutions options above 4K to be safe
                    continue;
                }

                // On non-4K streams, we force the resolution to never change unless it's above
                // 60 FPS, which may require a resolution reduction due to HDMI bandwidth limitations,
                // or it's a native resolution stream.
                if (prefConfig.width < 3840 && prefConfig.fps <= 60 && !isNativeResolutionStream) {
                    if (display.getMode().getPhysicalWidth() != candidate.getPhysicalWidth() ||
                            display.getMode().getPhysicalHeight() != candidate.getPhysicalHeight()) {
                        continue;
                    }
                }

                // Make sure the resolution doesn't regress unless if it's over 60 FPS
                // where we may need to reduce resolution to achieve the desired refresh rate.
                if (resolutionReduced && !(prefConfig.fps > 60 && resolutionFitsStream)) {
                    continue;
                }

                if (mayReduceRefreshRate() && refreshRateIsEqual && !isRefreshRateEqualMatch(candidate.getRefreshRate())) {
                    // If we had an equal refresh rate and this one is not, skip it. In min latency
                    // mode, we want to always prefer the highest frame rate even though it may cause
                    // microstuttering.
                    continue;
                }
                else if (refreshRateIsGood) {
                    // We've already got a good match, so if this one isn't also good, it's not
                    // worth considering at all.
                    if (!isRefreshRateGoodMatch(candidate.getRefreshRate())) {
                        continue;
                    }

                    if (mayReduceRefreshRate()) {
                        // User asked for the lowest possible refresh rate, so don't raise it if we
                        // have a good match already
                        if (candidate.getRefreshRate() > bestMode.getRefreshRate()) {
                            continue;
                        }
                    }
                    else {
                        // User asked for the highest possible refresh rate, so don't reduce it if we
                        // have a good match already
                        if (refreshRateReduced) {
                            continue;
                        }
                    }
                }
                else if (!isRefreshRateGoodMatch(candidate.getRefreshRate())) {
                    // We didn't have a good match and this match isn't good either, so just don't
                    // reduce the refresh rate.
                    if (refreshRateReduced) {
                        continue;
                    }
                } else {
                    // We didn't have a good match and this match is good. Prefer this refresh rate
                    // even if it reduces the refresh rate. Lowering the refresh rate can be beneficial
                    // when streaming a 60 FPS stream on a 90 Hz device. We want to select 60 Hz to
                    // match the frame rate even if the active display mode is 90 Hz.
                }

                bestMode = candidate;
                refreshRateIsGood = isRefreshRateGoodMatch(candidate.getRefreshRate());
                refreshRateIsEqual = isRefreshRateEqualMatch(candidate.getRefreshRate());
            }

            LimeLog.info("Best display mode: "+bestMode.getPhysicalWidth()+"x"+
                    bestMode.getPhysicalHeight()+"x"+bestMode.getRefreshRate());

            // Only apply new window layout parameters if we've actually changed the display mode
            if (display.getMode().getModeId() != bestMode.getModeId()) {
                // If we only changed refresh rate and we're on an OS that supports Surface.setFrameRate()
                // use that instead of using preferredDisplayModeId to avoid the possibility of triggering
                // bugs that can cause the system to switch from 4K60 to 4K24 on Chromecast 4K.
                if (prefConfig.useTextureView || Build.VERSION.SDK_INT < Build.VERSION_CODES.S ||
                        display.getMode().getPhysicalWidth() != bestMode.getPhysicalWidth() ||
                        display.getMode().getPhysicalHeight() != bestMode.getPhysicalHeight()) {
                    // Apply the display mode change
                    windowLayoutParams.preferredDisplayModeId = bestMode.getModeId();
                    getVideoWindow().setAttributes(windowLayoutParams);
                }
                else {
                    LimeLog.info("Using setFrameRate() instead of preferredDisplayModeId due to matching resolution");
                }
            }
            else {
                LimeLog.info("Current display mode is already the best display mode");
            }

            displayRefreshRate = bestMode.getRefreshRate();
        }
        // On L, we can at least tell the OS that we want a refresh rate
        else {
            float bestRefreshRate = display.getRefreshRate();
            for (float candidate : display.getSupportedRefreshRates()) {
                LimeLog.info("Examining refresh rate: "+candidate);

                if (candidate > bestRefreshRate) {
                    // Ensure the frame rate stays around 60 Hz for <= 60 FPS streams
                    if (prefConfig.fps <= 60) {
                        if (candidate >= 63) {
                            continue;
                        }
                    }

                    bestRefreshRate = candidate;
                }
            }

            LimeLog.info("Selected refresh rate: "+bestRefreshRate);
            windowLayoutParams.preferredRefreshRate = bestRefreshRate;
            displayRefreshRate = bestRefreshRate;

            // Apply the refresh rate change
            getVideoWindow().setAttributes(windowLayoutParams);
        }

        if (prefConfig.useTextureView && !useArr) {
            // A SurfaceTexture is composed into the window, so Surface.setFrameRate() cannot vote for it.
            windowLayoutParams.preferredRefreshRate = displayRefreshRate;
            getVideoWindow().setAttributes(windowLayoutParams);
        }

        if (!prefConfig.useTextureView && prefConfig.upscalingMode == UpscalingPolicy.Mode.OFF) {
            // Keep decoder buffers at stream resolution; the compositor scales them to the view.
            renderView.getHolder().setFixedSize(prefConfig.width, prefConfig.height);
        }
        applyVideoScaling();

        // Set the desired refresh rate that will get passed into setFrameRate() later
        desiredRefreshRate = displayRefreshRate;

        if (getPackageManager().hasSystemFeature(PackageManager.FEATURE_TELEVISION) ||
                getPackageManager().hasSystemFeature(PackageManager.FEATURE_LEANBACK)) {
            // TVs may take a few moments to switch refresh rates, and we can probably assume
            // it will be eventually activated.
            // TODO: Improve this
            return displayRefreshRate;
        }
        else {
            // Use the lower of the current refresh rate and the selected refresh rate.
            // The preferred refresh rate may not actually be applied (ex: Battery Saver mode).
            return Math.min(getVideoDisplay().getRefreshRate(), displayRefreshRate);
        }
    }

    @SuppressLint("InlinedApi")
    private final Runnable hideSystemUi = new Runnable() {
            @Override
            public void run() {
                View decor = getWindow().getDecorView();
                WindowInsetsControllerCompat controller = WindowCompat.getInsetsController(getWindow(), decor);

                // Keep one edge-to-edge coordinate space. AdaptiveLayout applies the
                // visible system-bar insets while windowed; fitting here as well would
                // subtract them twice, particularly after leaving fullscreen on API 29.
                if (isWindowedStream()) {
                    Game.this.getWindow().getDecorView().setSystemUiVisibility(
                            View.SYSTEM_UI_FLAG_LAYOUT_STABLE |
                            View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION |
                            View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN);
                    controller.show(WindowInsetsCompat.Type.systemBars());
                }
                else {
                    // Use immersive mode
                    Game.this.getWindow().getDecorView().setSystemUiVisibility(
                            View.SYSTEM_UI_FLAG_LAYOUT_STABLE |
                            View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION |
                            View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN |
                            View.SYSTEM_UI_FLAG_HIDE_NAVIGATION |
                            View.SYSTEM_UI_FLAG_FULLSCREEN |
                            View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY);
                    controller.setSystemBarsBehavior(WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
                    controller.hide(WindowInsetsCompat.Type.systemBars());
                }
            }
    };

    private void hideSystemUi(int delay) {
        Handler h = getWindow().getDecorView().getHandler();
        if (h != null) {
            h.removeCallbacks(hideSystemUi);
            h.postDelayed(hideSystemUi, delay);
        }
    }

    @Override
    @TargetApi(Build.VERSION_CODES.N)
    public void onMultiWindowModeChanged(boolean isInMultiWindowMode) {
        super.onMultiWindowModeChanged(isInMultiWindowMode);
        configureStreamWindow();
        if (decoderRenderer == null) {
            return;
        }

        // In multi-window, we don't want to use the full-screen layout
        // flag. It will cause us to collide with the system UI.
        // This function will also be called for PiP so we can cover
        // that case here too.
        if (isInMultiWindowMode) {
            getWindow().clearFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN);
        }
        else {
            getWindow().addFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN);
        }

        updatePipAutoEnter();
        scheduleReconfigureCheck();

        // Correct the system UI visibility flags
        hideSystemUi(50);
    }

    // Mid-stream reconfigure. A Rubylight 2.2 host that lists control message 0x5532 changes the
    // stream's size or frame rate in place and answers with an IDR. A native-resolution stream asks
    // for the panel's size when a foldable opens or closes, computed like the stream's start size,
    // and for its window's size in split screen or a desktop window. A stream at the panel's top
    // refresh rate follows a panel whose top rate changes. Fixed presets, PyroWave and decoders
    // without adaptive playback never ask.
    private void scheduleReconfigureCheck() {
        scheduleReconfigureCheck(StreamReconfigureRequester.SETTLE_MS);
    }

    private void scheduleReconfigureCheck(long settleMs) {
        if (!connected || isFinishing()) {
            return;
        }
        bitrateHandler.removeCallbacks(checkReconfigure);
        bitrateHandler.postDelayed(checkReconfigure, reconfigureRequester.onChange(SystemClock.uptimeMillis(), settleMs));
    }

    // Rotate on a host that cannot turn this stream: say once that the picture is scaled instead.
    private void noteRotationWithoutReconfigure() {
        if (streamOrientation != StreamOrientation.ROTATE || orientationNoticeShown || !connected ||
                externalPresentation != null || app == null || app.getRole() == NvApp.Role.INPUT_ONLY ||
                (hostShapesStream && MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_RECONFIGURE))) {
            return;
        }
        orientationNoticeShown = true;
        messages.alert(R.string.stream_orientation_scaled, Toast.LENGTH_LONG);
    }

    private void startReconfigureTracking() {
        displaySignature = displaySignature();
        if (decoderRenderer == null) {
            return;
        }
        int[] size = decoderRenderer.getStreamSize();
        streamStartedLandscape = size[0] >= size[1];
        int fpsMillihz = decoderRenderer.getStreamFrameRateMilliHz();
        if (fpsMillihz <= 0) fpsMillihz = streamFps * 1000;
        // What the phone offered when the stream was asked for, in the window shape and on the panel
        // it had then: a turn, fold or unfold while the stream connected was not seen, so the settled
        // check below still asks for it.
        boolean launchLandscape = launchWidth > 0 && launchHeight > 0 ? launchWidth >= launchHeight : windowLandscape();
        int[] offered = offeredStream(size[0], size[1], fpsMillihz, launchLandscape,
                launchPanel != null ? launchPanel : PreferenceConfiguration.nativeResolution(this));
        if (offered == null) {
            offered = new int[] {size[0], size[1], fpsMillihz};
        }
        reconfigureRequester.start(size[0], size[1], fpsMillihz, offered[0], offered[1], offered[2]);
        qualityMaxWidth = size[0];
        qualityMaxHeight = size[1];
        qualityMaxFps = streamFps;
        qualityMaxFrameRateMilliHz = fpsMillihz;
        // The surface was created before negotiation supplied the exact stream cadence.
        refreshSurfaceFrameRateVote();
        scheduleReconfigureCheck();
    }

    private void checkStreamReconfigure() {
        if (!connected || isFinishing() || decoderRenderer == null || (!nativeResolutionStream && !fpsFollowsDisplay &&
                !StreamOrientation.followsRotation(streamOrientation, hostShapesStream))) {
            return;
        }
        long now = SystemClock.uptimeMillis();
        if (!reconfigureRequester.settled(now)) {
            bitrateHandler.postDelayed(checkReconfigure, reconfigureRequester.remainingMs(now));
            return;
        }
        if ((Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && isInPictureInPictureMode()) ||
                !MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_RECONFIGURE)) {
            return;
        }
        if (adaptiveQuality != null && pendingQualityTarget != null) {
            bitrateHandler.postDelayed(checkReconfigure, 1000);
            return;
        }
        int[] offered = offeredStream(adaptiveQuality == null ? reconfigureRequester.width() : qualityMaxWidth,
                adaptiveQuality == null ? reconfigureRequester.height() : qualityMaxHeight,
                adaptiveQuality == null ? reconfigureRequester.fpsMillihz() : qualityCeilingFrameRateMilliHz());
        if (adaptiveQuality != null && offered != null) {
            AdaptiveQualityController.Target target = adaptiveQuality.resizeCeiling(offered[0], offered[1],
                    Math.round(offered[2] / 1000f), now);
            // The quality ladder uses whole FPS, but a fractional-only display change still
            // needs to reach the host when the current level uses the ceiling's rate.
            if (target == null && adaptiveQuality.current().fps == Math.round(offered[2] / 1000f) &&
                    reconfigureRequester.fpsMillihz() != offered[2]) target = adaptiveQuality.current();
            if (target == null || applyAdaptiveQuality(target, now, offered[2])) {
                qualityMaxWidth = offered[0]; qualityMaxHeight = offered[1]; qualityMaxFps = Math.round(offered[2] / 1000f);
                qualityMaxFrameRateMilliHz = offered[2];
                streamPanelHz = panelMaxRefreshRate;
            }
            return;
        }
        int[] request = offered == null ? null : reconfigureRequester.decide(offered[0], offered[1], offered[2]);
        if (request == null) {
            return;
        }
        if (requestStreamReconfigure(request)) {
            // This is the new ceiling even if automatic quality is enabled later.
            qualityMaxWidth = request[0]; qualityMaxHeight = request[1]; qualityMaxFps = Math.round(request[2] / 1000f);
            qualityMaxFrameRateMilliHz = request[2];
            streamPanelHz = panelMaxRefreshRate;
        }
    }

    private boolean requestStreamReconfigure(int[] request) {
        LimeLog.info("Reconfigure: asking for " + request[0] + "x" + request[1] + " at " + request[2] / 1000f +
                " fps (was " + reconfigureRequester.width() + "x" + reconfigureRequester.height() + " at " +
                reconfigureRequester.fpsMillihz() / 1000f + " fps)");
        if (!decoderRenderer.prepareForReconfigure(request[0], request[1])) {
            LimeLog.warning("Reconfigure: the decoder cannot switch to " + request[0] + "x" + request[1] +
                    "; the stream stays as it is");
            if (StreamOrientation.followsRotation(streamOrientation, hostShapesStream) && !orientationNoticeShown) {
                orientationNoticeShown = true;
                messages.alert(R.string.stream_orientation_decoder_limit, Toast.LENGTH_LONG);
            }
            return false;
        }
        if (!MoonBridge.requestReconfigure(request[0], request[1], request[2])) {
            LimeLog.warning("Reconfigure: request not sent");
            return false;
        }
        boolean fpsChanged = request[2] != reconfigureRequester.fpsMillihz();
        reconfigureRequester.sent(request);
        decoderRenderer.resetDecodeTiming();
        if (fpsChanged) setStreamFrameRate(request[2]);
        return true;
    }

    // The stream runs at this rate now: a reconfigure asked for it, or a 0x553C rate change ran.
    private void setStreamFrameRate(int fpsMillihz) {
        int fps = Math.round(fpsMillihz / 1000f);
        streamFps = fps;
        prefConfig.fps = fps;
        decoderRenderer.setStreamFrameRateMilliHz(fpsMillihz);
        if (adaptiveBitrate != null) adaptiveBitrate.setFrameRate(fps);
        refreshSurfaceFrameRateVote();
    }

    // What this phone offers a stream that follows it, {width, height, fps in millihertz}, given
    // the stream's current values; null when this stream cannot change in place.
    private int[] offeredStream(int width, int height, int fpsMillihz) {
        return offeredStream(width, height, fpsMillihz, windowLandscape(), PreferenceConfiguration.nativeResolution(this));
    }

    // As above, for a window of that orientation on a screen whose own resolution is nativeResolution.
    private int[] offeredStream(int width, int height, int fpsMillihz, boolean windowLandscape, String nativeResolution) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return null;
        }
        int[] limits = decoderRenderer.getReconfigureLimits();
        if (limits == null) {
            return null;
        }
        int[] size = nativeResolutionStream ? nativeStreamSize(limits, windowLandscape, nativeResolution) :
                StreamOrientation.followsRotation(streamOrientation, hostShapesStream) && !isInMultiWindowMode() ?
                StreamOrientation.turnedSize(launchWidth, launchHeight, windowLandscape, limits[0], limits[1],
                        limits[2], limits[3]) : null;
        int fps = StreamResizePolicy.targetFpsMillihz(fpsFollowsDisplay, fpsMillihz, streamPanelHz, panelMaxRefreshRate);
        return size == null ? new int[] {width, height, fps} : new int[] {size[0], size[1], fps};
    }

    @TargetApi(Build.VERSION_CODES.O)
    private int[] nativeStreamSize(int[] limits, boolean windowLandscape, String nativeResolution) {
        if (isInMultiWindowMode()) {
            // Split screen or a desktop window: the stream fills the window's pane, in panel pixels.
            View pane = findViewById(android.R.id.content);
            int paneWidth = pane.getWidth() - pane.getPaddingLeft() - pane.getPaddingRight();
            int paneHeight = pane.getHeight() - pane.getPaddingTop() - pane.getPaddingBottom();
            Display display = getWindowManager().getDefaultDisplay();
            Point real = new Point();
            display.getRealSize(real);
            // The screen in use's own long side: a foldable can list the other panel's modes too.
            String panel = PreferenceConfiguration.nativeResolution(this);
            int modeLong = panel == null ? 0 : Integer.parseInt(panel.substring(0, panel.indexOf('x')));
            float scale = modeLong > 0 && real.x > 0 && real.y > 0 ? (float) modeLong / Math.max(real.x, real.y) : 1;
            return StreamResizePolicy.nativeSize(paneWidth, paneHeight, scale, StreamResizePolicy.ORIENTATION_OF_AREA,
                    limits[0], limits[1], limits[2], limits[3]);
        }
        // Full screen: the same native resolution the stream starts with, in the orientation it
        // started in, even after a split-screen window turned it; with Rotate on a host that
        // draws the stream at its shape, in the window's orientation.
        if (nativeResolution == null) {
            return null;
        }
        String[] parts = nativeResolution.split("x");
        return StreamResizePolicy.nativeSize(Integer.parseInt(parts[0]), Integer.parseInt(parts[1]), 1,
                StreamOrientation.streamOrientation(streamOrientation, hostShapesStream, streamStartedLandscape,
                        windowLandscape),
                limits[0], limits[1], limits[2], limits[3]);
    }

    private boolean windowLandscape() {
        Point real = new Point();
        getWindowManager().getDefaultDisplay().getRealSize(real);
        return real.x >= real.y;
    }

    private String displaySignature() {
        Display display = getVideoDisplay();
        // The input pane must not hide a refresh-capability change on the video output.
        // Local output still tracks the phone's native size across folds and mode changes.
        String resolution = externalPresentation == null ? PreferenceConfiguration.nativeResolution(this) : "";
        return display.getDisplayId() + ":" + resolution + "@" +
                com.limelight.binding.video.DisplayFrameRatePolicy.maxRefreshRate(display);
    }

    // Display changes also fire for refresh rate switches; only a new panel or a new top refresh
    // rate matters here.
    private void onDisplayModesChanged() {
        // ARR support can change with the active mode even when size and maximum Hz do not.
        // Observe it independently of the selected policy; changing votes here could oscillate modes.
        logDisplayRefreshPolicy(getVideoDisplay());
        String signature = displaySignature();
        if (signature.equals(displaySignature)) {
            return;
        }
        LimeLog.info("Display modes changed: " + displaySignature + " -> " + signature);
        displaySignature = signature;
        configureStreamWindow();
        panelMaxRefreshRate = com.limelight.binding.video.DisplayFrameRatePolicy.maxRefreshRate(
                getVideoDisplay());
        scheduleReconfigureCheck();
    }

    private void logDisplayRefreshPolicy(Display display) {
        logDisplayRefreshPolicy(display,
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA && display.hasArrSupport());
    }

    private void logDisplayRefreshPolicy(Display display, boolean arrSupported) {
        WindowManager.LayoutParams attributes = getVideoWindow().getAttributes();
        String changed = displayRefreshDiagnostics.update(Build.VERSION.SDK_INT, display.getDisplayId(),
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.M ? display.getMode().getModeId() : 0,
                arrSupported, prefConfig.vrr, useArr,
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.M ? attributes.preferredDisplayModeId : 0,
                attributes.preferredRefreshRate, appSurfaceFrameRateVote);
        if (changed != null) LimeLog.info(changed);
    }

    private void onPaneLaidOut(View pane) {
        if (externalPresentation != null) return;
        int width = pane.getWidth() - pane.getPaddingLeft() - pane.getPaddingRight();
        int height = pane.getHeight() - pane.getPaddingTop() - pane.getPaddingBottom();
        if (width == lastPaneWidth && height == lastPaneHeight) {
            return;
        }
        lastPaneWidth = width;
        lastPaneHeight = height;
        // Full-screen sizes come from the display; only a window follows its own pane.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N && isInMultiWindowMode()) {
            scheduleReconfigureCheck();
        }
    }

    // Frames of a new size came out of the decoder: scale, letterbox, touch and picture-in-picture
    // now follow that size.
    private void applyStreamSize(int width, int height) {
        if (isFinishing() || isDestroyed() || (width == prefConfig.width && height == prefConfig.height)) {
            return;
        }
        LimeLog.info("Stream now " + width + "x" + height + " (was " + prefConfig.width + "x" + prefConfig.height + ")");
        prefConfig.width = width;
        prefConfig.height = height;
        if (adaptiveBitrate != null) adaptiveBitrate.setStreamSize(width, height);
        configureStreamWindow();
        if (prefConfig.upscalingMode == UpscalingPolicy.Mode.OFF) {
            if (prefConfig.useTextureView) {
                SurfaceTexture texture = renderView.getTextureView().getSurfaceTexture();
                if (texture != null) {
                    texture.setDefaultBufferSize(width, height);
                }
            } else {
                renderView.getHolder().setFixedSize(width, height);
            }
        }
        applyVideoScaling();
        if (prefConfig.touchscreenTrackpad && conn != null) {
            // Trackpad motion is scaled to the stream size.
            touchInput.cancelTouchInput();
            touchInput.initializeTouchContexts();
        }
        setPreferredOrientationForCurrentDisplay();
        updatePipAutoEnter();
    }

    @Override
    protected void onDestroy() {
        if (nativeMenuThread != null) nativeMenuThread.shutdownNow();
        if (streamHealthThread != null) streamHealthThread.shutdownNow();
        // A staged exit still waiting on the display ends here with the activity.
        exitHandler.removeCallbacksAndMessages(null);
        exitFinished = true;
        stopConnection();
        if (externalPresentation != null) {
            externalPresentation.setOnDismissListener(null);
            externalPresentation.dismiss();
            externalPresentation = null;
        }
        ((android.hardware.display.DisplayManager) getSystemService(DISPLAY_SERVICE))
                .unregisterDisplayListener(inputDisplayListener);
        SpinnerDialog.closeDialogs(this);
        Dialog.closeDialogs(this);
        getWindow().getDecorView().removeCallbacks(hideSystemUi);
        getWindow().getDecorView().removeCallbacks(toggleGrab);
        if (pipDisconnectIntent != null) {
            unregisterReceiver(pipReceiver);
            pipDisconnectIntent.cancel();
        }
        bitrateHandler.removeCallbacksAndMessages(null);
        keyboardPanHandler.removeCallbacks(keyboardPan);
        if (legacyKeyboard != null) legacyKeyboard.stop();

        if (controllerHandler != null) {
            controllerHandler.destroy();
        }
        if (keyboardTranslator != null) {
            InputManager inputManager = (InputManager) getSystemService(Context.INPUT_SERVICE);
            inputManager.unregisterInputDeviceListener(keyboardTranslator);
            inputManager.unregisterInputDeviceListener(pcKeyboardDeviceListener);
        }

        if (usbDriverServiceBound) {
            unbindService(usbDriverServiceConnection);
            usbDriverServiceBound = false;
        }

        // Destroy the capture provider
        if (inputCaptureProvider != null) {
            inputCaptureProvider.destroy();
        }
        LimeLog.info("Stream activity destroyed");
        super.onDestroy();
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        // singleTask can deliver a new shortcut launch to a stream still visible in PiP.
        touchInput.setInputGrabState(false);
        stopConnection();
        finishNow();
        startActivity(new Intent(intent));
    }

    @Override
    protected void onStart() {
        super.onStart();
        if (adaptiveLayout != null) adaptiveLayout.start();
    }

    @Override
    protected void onResume() {
        super.onResume();
        foreground = true;
        if (resumeStoppedStreamIfNeeded()) {
            recoveryCancelledOnPause = false;
            restoreInputOnResume = false;
            return;
        }
        if (recoveryCancelledOnPause && !connected && !isFinishing()) {
            displayedFailureDialog = true;
            stopConnection();
            if (spinner != null) { spinner.dismiss(); spinner = null; }
            showReconnectDialog(R.string.conn_terminated_title, getString(R.string.stream_recovery_paused));
        }
        recoveryCancelledOnPause = false;
        if (restoreInputOnResume && connected) {
            touchInput.setInputGrabState(true);
        }
        restoreInputOnResume = false;
        overlays.updateHardwareMetricsPolling();
    }

    @Override
    protected void onPause() {
        touchInput.releaseDesktopInput();
        if (trackpadPointer != null) trackpadPointer.stopMomentum();
        if (gameTouchControls != null) gameTouchControls.releaseInput();
        // Shown again on the next stream unless it was read.
        overlays.dismissStreamMenuHint(false);
        foreground = false;
        recoveryCancelledOnPause = recoveryHandoffInProgress || pendingRecovery != null || (isRecoveryAttempt() && connecting);
        cancelAutomaticRecovery();
        overlays.stopHardwareMetricsPolling();
        foregroundGeneration++;
        streamMenuAction = null;
        windowFocusActions.clear();
        getWindow().getDecorView().removeCallbacks(runWindowFocusActions);
        if (pendingConfirmation != null) {
            pendingConfirmation.cancel();
            launchConfirmationFinished();
        }
        if (streamMenu != null) {
            restoreInputOnResume = restoreInputAfterMenu;
            streamMenu.dismiss();
        }
        if (isFinishing()) {
            // Stop any further input device notifications before we lose focus (and pointer capture)
            if (controllerHandler != null) {
                controllerHandler.stop();
            }

            // Ungrab input to prevent further input device notifications
            touchInput.setInputGrabState(false);
        }

        super.onPause();
    }

    @Override
    protected void onStop() {
        super.onStop();
        if (adaptiveLayout != null) adaptiveLayout.stop();
        rememberStreamForResume(true);

        if (streamMenu != null) {
            streamMenu.dismiss();
        }

        SpinnerDialog.closeDialogs(this);
        Dialog.closeDialogs(Game.this);

        if (virtualController != null) {
            virtualController.hide();
        }
        if (modernController != null) {
            modernController.finishEditing(false);
            modernController.hide();
        }
        if (micButton != null) micButton.setSuppressed(true);

        if (conn != null) {
            int videoFormat = decoderRenderer.getActiveVideoFormat();

            displayedFailureDialog = true;
            stopConnection();

            if (prefConfig.enableLatencyToast) {
                int averageEndToEndLat = decoderRenderer.getAverageEndToEndLatency();
                int averageDecoderLat = decoderRenderer.getAverageDecoderLatency();
                String message = null;
                if (averageEndToEndLat > 0) {
                    message = getResources().getString(R.string.conn_client_latency)+" "+averageEndToEndLat+" ms";
                    if (averageDecoderLat > 0) {
                        message += " ("+getResources().getString(R.string.conn_client_latency_hw)+" "+averageDecoderLat+" ms)";
                    }
                }
                else if (averageDecoderLat > 0) {
                    message = getResources().getString(R.string.conn_hardware_latency)+" "+averageDecoderLat+" ms";
                }

                // Add the video codec to the post-stream toast
                if (message != null) {
                    message += " [";

                    if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_H264) != 0) {
                        message += "H.264";
                    }
                    else if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_H265) != 0) {
                        message += "HEVC";
                    }
                    else if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_AV1) != 0) {
                        message += "AV1";
                    }
                    else if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
                        message += "PyroWave";
                    }
                    else {
                        message += "UNKNOWN";
                    }

                    if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0) {
                        message += streamSdr10 && !Boolean.TRUE.equals(decoderRenderer.getHostHdrReport()) ?
                                " 10-bit SDR" : " HDR";
                    }

                    message += "]";
                }

                if (message != null) {
                    messages.alert(message, Toast.LENGTH_LONG);
                }
            }

            // Clear the tombstone count if we terminated normally
            synchronized (tombstonePrefs) {
                if (!reportedCrash && tombstonePrefs.getInt("CrashCount", 0) != 0) {
                    tombstonePrefs.edit()
                            .putInt("CrashCount", 0)
                            .putInt("LastNotifiedCrashCount", 0)
                            .remove("CrashHostUuid")
                            .apply();
                }
            }
        }

        if (finishesWhenStopped()) finish();
    }

    @Override
    public void onBackPressed() {
        if (modernController != null && modernController.isEditing()) {
            // Back leaves the layout editor without keeping its changes, as Cancel does.
            modernController.finishEditing(false);
        } else if (connected && keyboardInput.isPcKeyboardVisible()) {
            keyboardInput.hidePcKeyboard();
        } else if (connected) {
            showStreamMenu();
        } else {
            super.onBackPressed();
        }
    }

    @Override
    protected void attachBaseContext(Context base) {
        super.attachBaseContext(UiHelper.localeContext(base));
    }

    void showStreamDialog(android.app.Dialog dialog) {
        keyboardInput.hidePcKeyboard();
        touchInput.setInputGrabState(false);
        suppressPipRefCount++;
        updatePipAutoEnter();
        streamMenu = dialog;
        dialog.setOnDismissListener(ignored -> {
            if (streamMenu == dialog) {
                streamMenu = null;
            }
            if (streamMenuSheet != null && streamMenuSheet.sheet() == dialog) {
                streamMenuSheet = null;
            }
            suppressPipRefCount--;
            updatePipAutoEnter();
            Runnable action = streamMenuAction;
            streamMenuAction = null;
            if (foreground && connected && !isFinishing()) {
                windowFocusActions.add(() -> {
                    touchInput.setInputGrabState(restoreInputAfterMenu);
                    hideSystemUi(1000);
                    if (action != null) {
                        action.run();
                    }
                }, SystemClock.uptimeMillis());
                View decorView = getWindow().getDecorView();
                decorView.postDelayed(runWindowFocusActions, WindowFocusActionQueue.TIMEOUT_MS);
                if (hasWindowFocus()) {
                    decorView.post(runWindowFocusActions);
                }
            }
        });
        ActionSheet.keepSystemBarsHidden(dialog, this);
        dialog.show();
    }

    /** The controller quit combo: leaves the stream like the menu's Disconnect. */
    public void leaveStreamFromController() {
        if (!foreground || isFinishing()) {
            return;
        }
        leaveStream();
    }

    public void showStreamMenu() {
        if (!foreground || !connected || streamMenu != null) {
            return;
        }
        restoreInputAfterMenu = grabbedInput;
        // Whoever opens the menu knows how; the first-stream hint has done its job.
        overlays.dismissStreamMenuHint(true);
        // The audio switches show the host's state as it is now, so they appear once it answers.
        hostAudio = null;
        hostAudioUnread = false;
        streamMenuSheet = new com.limelight.ui.StreamMenu(this, this::streamMenuOptions, this::onStreamMenuItem,
                nativeMenuController());
        showStreamDialog(streamMenuSheet.sheet());
        refreshHostAudio();
    }

    // Night mode and headphone surround as the host last answered, for the stream menu; null until it does. UI thread.
    private com.limelight.nativeapi.NativeAudio hostAudio;
    // The host offers the switches but its answer didn't come: the menu says so instead of leaving them out silently.
    private boolean hostAudioUnread;
    private boolean hostAudioBusy;

    private boolean offersHostAudio() {
        return conn != null && app.getRole() == NvApp.Role.STREAM &&
                (conn.offersHostAudio(com.limelight.nativeapi.NativeCapabilities.NIGHT_MODE) ||
                        conn.offersHostAudio(com.limelight.nativeapi.NativeCapabilities.HEADPHONE_SURROUND));
    }

    // Reads the host's audio switches on the native menu thread; the open menu shows them once it answers.
    private void refreshHostAudio() {
        if (!offersHostAudio()) return;
        NvConnection connection = conn;
        int generation = foregroundGeneration;
        try {
            nativeMenuThread().execute(() -> {
                com.limelight.nativeapi.NativeAudio audio = null;
                try {
                    audio = connection.hostAudio();
                } catch (IOException e) {
                    LimeLog.warning("Host audio not read: " + e.getMessage());
                }
                com.limelight.nativeapi.NativeAudio answer = audio;
                runOnUiThread(() -> showHostAudio(answer, generation));
            });
        } catch (java.util.concurrent.RejectedExecutionException ignored) {
            // The stream is over.
        }
    }

    private void showHostAudio(com.limelight.nativeapi.NativeAudio audio, int generation) {
        if (generation != foregroundGeneration || isFinishing()) return;
        hostAudio = audio;
        hostAudioUnread = audio == null;
        if (streamMenuSheet != null) streamMenuSheet.refresh();
    }

    /**
     * A quick toggle for one of the host's audio switches: the host applies it to this stream from its next audio
     * packet, and the menu shows the host's answer rather than guessing. A refusal is shown with the host's message.
     */
    private void toggleHostAudio(String id) {
        com.limelight.nativeapi.NativeAudio audio = hostAudio;
        com.limelight.nativeapi.NativeAudio.Setting setting = audio == null ? null : audio.get(id);
        if (setting == null || !setting.switchable() || hostAudioBusy || conn == null) return;
        boolean on = !setting.value;
        NvConnection connection = conn;
        int generation = foregroundGeneration;
        hostAudioBusy = true;
        try {
            nativeMenuThread().execute(() -> {
                com.limelight.nativeapi.NativeAudio answer = null;
                IOException failure = null;
                try {
                    answer = connection.writeHostAudio(id, on);
                } catch (IOException e) {
                    failure = e;
                }
                com.limelight.nativeapi.NativeAudio changed = answer;
                IOException refused = failure;
                runOnUiThread(() -> {
                    hostAudioBusy = false;
                    if (refused != null) {
                        LimeLog.warning("Host audio not changed: " + refused.getMessage());
                        if (foreground && generation == foregroundGeneration && connected && !isFinishing()) {
                            messages.alert(getString(R.string.stream_audio_failed, refused.getMessage()), Toast.LENGTH_LONG);
                        }
                        // Show what the host has instead of the switch's old state.
                        refreshHostAudio();
                        return;
                    }
                    showHostAudio(changed, generation);
                    com.limelight.nativeapi.NativeAudio.Setting now = changed.get(id);
                    // Why the stream doesn't apply it yet, in the host's words unless it is just the next stream.
                    String note = now == null || now.reason == null ? null :
                            com.limelight.nativeapi.NativeAudio.NEXT_STREAM.equals(now.reason) ?
                                    getString(R.string.stream_audio_next_stream) : now.message;
                    if (on && note != null && foreground && generation == foregroundGeneration) {
                        messages.alert(note, Toast.LENGTH_LONG);
                    }
                });
            });
        } catch (java.util.concurrent.RejectedExecutionException ignored) {
            // The stream is over.
            hostAudioBusy = false;
        }
    }

    private com.limelight.console.NativeMenuController nativeMenuController;
    private java.util.concurrent.ScheduledExecutorService nativeMenuThread;

    // The Radeon panel's reads and writes: one background thread, never the render, decode or input threads.
    private com.limelight.console.NativeMenuController nativeMenuController() {
        if (conn == null || !conn.hasNativeSessionActions() || app.getRole() != NvApp.Role.STREAM) {
            return null;
        }
        if (nativeMenuController == null) {
            java.util.concurrent.ScheduledExecutorService thread = nativeMenuThread();
            Handler main = new Handler(Looper.getMainLooper());
            nativeMenuController = new com.limelight.console.NativeMenuController(conn::nativeApiClient,
                    (task, delayMs) -> {
                        try {
                            thread.schedule(task, delayMs, java.util.concurrent.TimeUnit.MILLISECONDS);
                        } catch (java.util.concurrent.RejectedExecutionException ignored) {
                            // The stream is over.
                        }
                    }, main::post);
        }
        return nativeMenuController;
    }

    private java.util.concurrent.ScheduledExecutorService nativeMenuThread() {
        if (nativeMenuThread == null) {
            nativeMenuThread = java.util.concurrent.Executors.newSingleThreadScheduledExecutor(task -> {
                Thread thread = new Thread(task, "Native menu");
                thread.setDaemon(true);
                return thread;
            });
        }
        return nativeMenuThread;
    }

    /**
     * Opens the menu the host draws into the stream. Like every menu action it runs once Nightjar's own sheet has
     * closed, so input reaches the stream (and the host's menu) again; the request runs on the native menu thread and
     * a refusal is shown with the host's message.
     */
    private void openHostMenu() {
        if (conn == null || !connected) return;
        NvConnection connection = conn;
        int generation = foregroundGeneration;
        try {
            nativeMenuThread().execute(() -> {
                try {
                    connection.openHostMenu();
                } catch (IOException e) {
                    LimeLog.warning("Host menu not opened: " + e.getMessage());
                    showHostActionError(e, generation);
                }
            });
        } catch (java.util.concurrent.RejectedExecutionException ignored) {
            // The stream is over.
        }
    }

    // What the stream menu offers right now. Host features and permissions decide the rest.
    private com.limelight.ui.StreamMenu.Options streamMenuOptions() {
        com.limelight.ui.StreamMenu.Options options = new com.limelight.ui.StreamMenu.Options(app.getRole(), conn.getHostDetails());
        options.canSendInput = conn.canSendInput();
        options.nativeHostControl = conn.hasNativeHostControl();
        options.hostMenu = conn.hasNativeCouchMenu();
        options.usbForwarding = prefConfig.usbForwarding && MoonBridge.isRustStream() && conn.offersUsbForwarding();
        options.imageClipboard = conn.hasImageClipboard();
        options.microphoneOffered = conn.getMicrophoneStatus().canMute();
        options.microphoneMuted = conn.isMicrophoneMuted();
        options.nightModeOffered = conn.offersHostAudio(com.limelight.nativeapi.NativeCapabilities.NIGHT_MODE);
        options.headphoneSurroundOffered = conn.offersHostAudio(com.limelight.nativeapi.NativeCapabilities.HEADPHONE_SURROUND);
        options.hostAudio = hostAudio;
        options.hostAudioUnread = hostAudioUnread;
        options.micButtonAvailable = micButton != null;
        options.micButtonShown = micButton != null && prefConfig.micButton;
        options.overlayShown = prefConfig.enablePerfOverlay;
        options.overlayMode = overlayMode;
        options.overlayPosition = overlayCorner.key;
        options.localOutput = externalPresentation == null;
        options.mouseConnected = touchInput.hasExternalMouse();
        options.controllerMouseAvailable = controllerHandler != null && controllerHandler.hasControllers();
        options.controllerMouseOn = controllerHandler != null && controllerHandler.isMouseEmulationActive();
        options.cursorVisible = cursorVisible;
        options.onscreenControls = prefConfig.onscreenController;
        options.gameControls = gameTouchControls != null;
        options.keyboardKeysShown = keyboardInput.isKeyboardOverlayVisible();
        options.scalingAvailable = canScaleLive();
        options.zoomEnabled = desktopGestures != null && desktopGestures.enabled();
        options.pictureInPicture = canEnterPictureInPicture();
        options.externalDisplays = hasPresentationDisplays();
        options.automaticQuality = supportsAdaptiveQuality();
        options.automaticBitrateOn = isAdaptiveBitrateEnabled() || isRateChangeBitrateOn();
        options.rateChange = rateChanges != null;
        options.rateChangeBitrate = rateChanges != null && prefConfig.rateChangeBitrate;
        options.webUiAddress = hostWebUiUrl() != null;
        options.connection = connectionInfo();
        return options;
    }

    // What the native page says about the stream; values the stream already holds, nothing read from the network.
    private com.limelight.console.ConnectionInfo connectionInfo() {
        if (decoderRenderer == null || prefConfig == null) return null;
        ComputerDetails details = conn.getHostDetails();
        return new com.limelight.console.ConnectionInfo(details.name, decoderRenderer.getActiveVideoFormat(),
                prefConfig.width, prefConfig.height, prefConfig.fps, currentBitrate == 0 ? prefConfig.bitrate : currentBitrate,
                wifiDiagnostics != null ? wifiDiagnostics.latest() : null,
                com.limelight.console.ConnectionInfo.rttMs(MoonBridge.getEstimatedRttInfo()));
    }

    private void onStreamMenuItem(com.limelight.ui.StreamMenu.Item item) {
        switch (item) {
            // The quick toggles change in place and leave the menu open.
            case OVERLAY: overlays.togglePerformanceOverlay(); return;
            case MICROPHONE: toggleMicrophoneMute(); return;
            case NIGHT_MODE: toggleHostAudio(com.limelight.nativeapi.NativeCapabilities.NIGHT_MODE); return;
            case HEADPHONE_SURROUND: toggleHostAudio(com.limelight.nativeapi.NativeCapabilities.HEADPHONE_SURROUND); return;
            default: break;
        }
        Runnable action = null;
        ComputerDetails details = conn.getHostDetails();
        switch (item) {
            case PC_KEYBOARD: action = keyboardInput::togglePcKeyboard; break;
            case KEYBOARD: action = this::toggleKeyboard; break;
            case TOUCH_MODE: action = touchInput::showTouchModeDialog; break;
            case MOUSE_CURSOR: action = touchInput::toggleCursorVisibility; break;
            case CONTROLLER_MOUSE: action = () -> { if (controllerHandler != null) controllerHandler.toggleMouseEmulationFromMenu(); }; break;
            case ONSCREEN_CONTROLS: action = () -> touchInput.setOnscreenControlsEnabled(!prefConfig.onscreenController); break;
            case CONTROLS_LAYOUT: action = touchInput::showControllerLayoutDialog; break;
            case MIC_BUTTON: action = () -> touchInput.setMicButtonShown(!prefConfig.micButton); break;
            case GAME_CONTROLS: action = gameTouchControls == null ? null : gameTouchControls::showMenu; break;
            case KEYBOARD_KEYS: action = keyboardInput::toggleKeyboardOverlay; break;
            case VIDEO_SCALING: action = this::showVideoScalingDialog; break;
            case ZOOM: action = this::toggleDesktopZoom; break;
            case ZOOM_RESET: action = () -> {
                desktopGestures.reset();
                if (trackpadPointer != null) trackpadPointer.syncCursor();
            }; break;
            case PICTURE_IN_PICTURE: action = this::enterPictureInPictureNow; break;
            case VIDEO_OUTPUT: action = this::showVideoOutputDialog; break;
            case BITRATE: action = this::showBitrateDialog; break;
            case RATE_CHANGE: action = this::showRateChangeDialog; break;
            case AUTOMATIC_QUALITY: action = this::showAdaptiveQualityDialog; break;
            case AUTOMATIC_BITRATE: action = this::toggleAdaptiveBitrateFromMenu; break;
            case SERVER_COMMANDS: action = () -> showServerCommands(details); break;
            case CLIPBOARD_SEND: action = () -> transferClipboard(true); break;
            case CLIPBOARD_RECEIVE: action = () -> transferClipboard(false); break;
            case USB_DEVICES: action = this::showUsbDevices; break;
            case IMAGE_CLIPBOARD_SEND: action = () -> transferClipboardImage(true); break;
            case IMAGE_CLIPBOARD_RECEIVE: action = () -> transferClipboardImage(false); break;
            case FILES: action = this::showFiles; break;
            case HOST_WEB_UI: action = this::showHostWebUi; break;
            case HOST_STATUS: action = this::refreshHostStatus; break;
            case HOST_MENU: action = this::openHostMenu; break;
            case RECONNECT: action = this::reconnectStream; break;
            case DISCONNECT: action = this::leaveStream; break;
            case QUIT_GAME: action = this::confirmQuitApp; break;
            case END_HOST_SESSION: action = this::confirmEndHostSession; break;
            case CLOSE_GAME: action = this::confirmCloseGame; break;
            case END_SESSION: action = this::endRemoteSession; break;
            case OVERLAY_OFF: action = overlays::hidePerformanceOverlay; break;
            case OVERLAY_TOP_LEFT: action = () -> overlays.setPerformanceOverlayPosition(com.limelight.ui.OverlayPlacement.Corner.TOP_LEFT); break;
            case OVERLAY_TOP_RIGHT: action = () -> overlays.setPerformanceOverlayPosition(com.limelight.ui.OverlayPlacement.Corner.TOP_RIGHT); break;
            case OVERLAY_BOTTOM_LEFT: action = () -> overlays.setPerformanceOverlayPosition(com.limelight.ui.OverlayPlacement.Corner.BOTTOM_LEFT); break;
            case OVERLAY_BOTTOM_RIGHT: action = () -> overlays.setPerformanceOverlayPosition(com.limelight.ui.OverlayPlacement.Corner.BOTTOM_RIGHT); break;
            case OVERLAY_COMPACT: action = () -> overlays.showPerformanceOverlay(PerformanceOverlay.MODE_COMPACT); break;
            case OVERLAY_STATISTICS: action = () -> overlays.showPerformanceOverlay(PerformanceOverlay.MODE_STATISTICS); break;
            case OVERLAY_DEBUG: action = () -> overlays.showPerformanceOverlay(PerformanceOverlay.MODE_DEBUG); break;
            case OVERLAY_COPY: action = overlays::copyPerformanceStats; break;
            case STREAM_HEALTH: action = this::showStreamHealth; break;
            case LATENCY_TEST: action = this::runLatencyTest; break;
            default: break;
        }
        // Like every menu action, it runs once the menu has closed and the stream has focus again.
        streamMenuAction = action;
        if (streamMenuSheet != null) streamMenuSheet.sheet().dismiss();
    }

    /**
     * A USB-driver controller produces no Android key events, so its D-pad, A and B reach the open
     * stream menu or dialog from here: B goes back, A chooses, the D-pad moves the focus.
     */
    public void steerStreamMenu(int keyCode) {
        android.app.Dialog dialog = streamMenu;
        if (dialog == null || !dialog.isShowing() || dialog.getWindow() == null) return;
        if (keyCode == KeyEvent.KEYCODE_BUTTON_B) {
            dialog.onBackPressed();
            return;
        }
        View decor = dialog.getWindow().getDecorView();
        View focused = decor.findFocus();
        if (focused == null) {
            // After touch nothing has focus: the first press shows it, as a D-pad would, and chooses nothing.
            decor.requestFocusFromTouch();
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) decor.restoreDefaultFocus();
            return;
        }
        if (keyCode == KeyEvent.KEYCODE_DPAD_CENTER && focused.isClickable() && !(focused instanceof android.widget.AdapterView)) {
            focused.performClick();
            return;
        }
        long now = SystemClock.uptimeMillis();
        KeyEvent down = new KeyEvent(now, now, KeyEvent.ACTION_DOWN, keyCode, 0);
        // Lists and buttons handle their own keys; plain rows need the focus search the window would do.
        boolean handled = dialog.dispatchKeyEvent(down);
        dialog.dispatchKeyEvent(KeyEvent.changeAction(down, KeyEvent.ACTION_UP));
        if (handled) return;
        View next = null;
        switch (keyCode) {
            case KeyEvent.KEYCODE_DPAD_UP: next = focused.focusSearch(View.FOCUS_UP); break;
            case KeyEvent.KEYCODE_DPAD_DOWN: next = focused.focusSearch(View.FOCUS_DOWN); break;
            case KeyEvent.KEYCODE_DPAD_LEFT: next = focused.focusSearch(View.FOCUS_LEFT); break;
            case KeyEvent.KEYCODE_DPAD_RIGHT: next = focused.focusSearch(View.FOCUS_RIGHT); break;
            default: break;
        }
        if (next != null) next.requestFocus();
    }

    private void toggleAdaptiveBitrateFromMenu() {
        // Automatic bitrate on this device turns on and off for this stream only; the setting stays.
        if (isRateChangeBitrateOn() || (!isAdaptiveBitrateEnabled() && rateChanges != null && prefConfig.rateChangeBitrate)) {
            boolean enable = !isRateChangeBitrateOn();
            setRateChangeBitrateEnabled(enable);
            // The setting's own explanation, as the settings screen shows it.
            if (enable) messages.alert(R.string.summary_rate_change_bitrate, Toast.LENGTH_LONG);
            return;
        }
        boolean enable = !isAdaptiveBitrateEnabled();
        if (!enable || !hostActionInProgress) {
            setAdaptiveBitrateEnabled(enable, true);
            if (enable) {
                messages.alert(pyroWaveBitrate != null ? R.string.stream_pyrowave_bitrate_help :
                        R.string.stream_auto_bitrate_help, Toast.LENGTH_LONG);
            }
        } else {
            messages.alert(R.string.stream_bitrate_busy, Toast.LENGTH_SHORT);
        }
    }

    // The host web UI's address for this stream: the address and HTTPS port the stream connected to.
    private okhttp3.HttpUrl hostWebUiUrl() {
        ComputerDetails details = conn.getHostDetails();
        String address = getIntent().getStringExtra(EXTRA_HOST);
        if (details == null || address == null) return null;
        ComputerDetails target = new ComputerDetails();
        target.state = ComputerDetails.State.ONLINE;
        target.caps = details.caps;
        target.activeAddress = new ComputerDetails.AddressTuple(address,
                getIntent().getIntExtra(EXTRA_PORT, NvHTTP.DEFAULT_HTTP_PORT));
        target.httpsPort = details.httpsPort != 0 ? details.httpsPort : getIntent().getIntExtra(EXTRA_HTTPS_PORT, 0);
        return ServerHelper.getHostSettingsUrl(target);
    }

    private void showHostWebUi() {
        okhttp3.HttpUrl url = hostWebUiUrl();
        if (url == null) return;
        AlertDialog dialog = new NavigationDialogBuilder(this).setTitle(R.string.host_administration_menu)
                .setMessage(getString(R.string.host_administration_help, url.toString()) + "\n\n" +
                        getString(R.string.stream_web_ui_leaves))
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.host_administration_open, (prompt, which) -> streamMenuAction = () -> {
                    try {
                        startActivity(new Intent(Intent.ACTION_VIEW, android.net.Uri.parse(url.toString())));
                    } catch (android.content.ActivityNotFoundException | SecurityException e) {
                        messages.alert(R.string.host_administration_no_browser, Toast.LENGTH_LONG);
                    }
                }).create();
        showStreamDialog(dialog);
        TextView message = dialog.findViewById(android.R.id.message);
        if (message != null) message.setTextIsSelectable(true);
    }

    /** The PC's file transfer screen; it opens over the stream, so the menu says it leaves it first. */
    private void showFiles() {
        String uuid = getIntent().getStringExtra(EXTRA_PC_UUID);
        if (uuid == null) return;
        showStreamDialog(new NavigationDialogBuilder(this).setTitle(R.string.files_menu)
                .setMessage(R.string.files_stream_leaves)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.files_open, (prompt, which) -> streamMenuAction = () ->
                        startActivity(com.limelight.files.FileTransferActivity.forPc(this, uuid)))
                .create());
    }

    private final com.limelight.binding.audio.MicNotice.Once microphoneNotice = new com.limelight.binding.audio.MicNotice.Once();

    // The microphone settings ask for processing this device lacks: say so in the stream, once per difference.
    // A microphone that failed and was muted is said every time, since each unmute tries again.
    private void showMicrophoneNotice() {
        if (conn == null || isFinishing() || isDestroyed()) return;
        if (conn.takeMicrophoneFailure()) {
            messages.alert(getString(R.string.mic_failed), Toast.LENGTH_LONG);
            if (micButton != null) micButton.refresh();
        }
        String text = microphoneNotice.next(com.limelight.binding.audio.MicNotice.text(this, conn.takeMicrophoneNotice()));
        if (text != null) messages.alert(text, Toast.LENGTH_LONG);
    }

    private void toggleMicrophoneMute() {
        if (conn == null) return;
        conn.setMicrophoneMuted(!conn.isMicrophoneMuted());
        if (micButton != null) micButton.refresh();
    }

    private boolean latencyTestRunning;

    private void showStreamHealth() {
        com.limelight.utils.StreamHealth.Report report = streamHealth.report(SystemClock.uptimeMillis());
        StringBuilder text = healthText(this, report, wifiText(0));
        boolean manual = healthTrialAllowed(isAdaptiveBitrateEnabled(), isRateChangeBitrateOn(), adaptiveQuality != null);
        boolean undo = healthOriginalBitrate > 0 && currentBitrate == healthTrialBitrate && manual;
        boolean suggest = report.canSuggestBitrate() && supportsAdaptiveBitrate() && manual &&
                currentBitrate > 500 && !hostActionInProgress && !undo;
        if (undo || suggest) text.append("\n\n").append(getString(R.string.health_trial_help));
        MaterialAlertDialogBuilder builder = new NavigationDialogBuilder(this).setTitle(R.string.health_title)
                .setMessage(text).setNegativeButton(android.R.string.cancel, null)
                .setNeutralButton(R.string.overlay_copy, (dialog, which) -> {
                    ClipboardManager clipboard = (ClipboardManager) getSystemService(CLIPBOARD_SERVICE);
                    clipboard.setPrimaryClip(ClipData.newPlainText(getString(R.string.health_title), text));
                });
        if (undo || suggest) builder.setPositiveButton(undo ? R.string.health_undo : R.string.health_try,
                (dialog, which) -> applyHealthBitrate(undo));
        else builder.setPositiveButton(android.R.string.ok, null);
        showStreamDialog(builder.create());
    }

    String wifiText(long stallPeriodMs) {
        com.limelight.utils.WifiDiagnostics.Lock lock = Build.VERSION.SDK_INT < Build.VERSION_CODES.Q ?
                com.limelight.utils.WifiDiagnostics.Lock.UNSUPPORTED :
                lowLatencyWifiLock != null && lowLatencyWifiLock.isHeld() ?
                        com.limelight.utils.WifiDiagnostics.Lock.HELD : com.limelight.utils.WifiDiagnostics.Lock.NOT_HELD;
        return com.limelight.utils.WifiDiagnostics.describe(this,
                wifiDiagnostics != null ? wifiDiagnostics.latest() : null, lock, stallPeriodMs);
    }

    /** The stream doctor's findings, evidence and connection facts; shared with the debug fixture. */
    static StringBuilder healthText(Context context, com.limelight.utils.StreamHealth.Report report, String connection) {
        StringBuilder text = new StringBuilder(context.getString(R.string.health_intro, report.samples));
        if (!report.recent || report.samples < 3) text.append("\n\n").append(context.getString(R.string.health_insufficient));
        else if (report.findings.isEmpty()) text.append("\n\n").append(context.getString(R.string.health_no_issue));
        int[] descriptions = {R.string.health_network, R.string.health_decoder, R.string.health_host,
                R.string.health_presentation, R.string.health_heat};
        for (com.limelight.utils.StreamHealth.Finding finding : report.findings) {
            text.append("\n\n").append(finding == com.limelight.utils.StreamHealth.Finding.PERIODIC ?
                    context.getString(R.string.health_periodic,
                            com.limelight.utils.WifiDiagnostics.period(context, report.stallPeriodMs), report.stalls) :
                    finding == com.limelight.utils.StreamHealth.Finding.BURSTS ?
                    context.getString(R.string.health_bursts, report.lateSamples, report.samples) :
                    context.getString(descriptions[finding.ordinal()]));
        }
        text.append("\n\n").append(context.getString(
                report.hostIncludesSend ? R.string.health_evidence_send : R.string.health_evidence,
                healthMeasurement(context, report.worstLossPercent), healthMeasurement(context, report.worstDecodeMs),
                healthMeasurement(context, report.worstHostP95Ms), healthMeasurement(context, report.worstDisplayP95Ms)));
        text.append("\n\n").append(context.getString(R.string.wifi_heading)).append('\n').append(connection);
        return text;
    }

    private static String healthMeasurement(Context context, float value) {
        return Float.isFinite(value) && value >= 0 ? String.format(Locale.getDefault(), "%.1f", value) :
                context.getString(R.string.latency_unavailable);
    }

    /**
     * The doctor's bitrate trial runs only while nothing automatic owns the bitrate: the host's
     * adaptive bitrate, automatic bitrate on this device (0x553C) or adaptive quality.
     */
    static boolean healthTrialAllowed(boolean adaptiveBitrate, boolean rateChangeBitrate, boolean adaptiveQuality) {
        return !adaptiveBitrate && !rateChangeBitrate && !adaptiveQuality;
    }

    private void applyHealthBitrate(boolean undo) {
        if (!foreground || !connected || hostActionInProgress || !supportsAdaptiveBitrate() ||
                !healthTrialAllowed(isAdaptiveBitrateEnabled(), isRateChangeBitrateOn(), adaptiveQuality != null)) return;
        if (undo && (healthOriginalBitrate <= 0 || currentBitrate != healthTrialBitrate)) return;
        final int original = healthOriginalBitrate;
        final int expectedTrial = healthTrialBitrate;
        final int generation = foregroundGeneration;
        final NvConnection connection = conn;
        hostActionInProgress = true;
        new Thread(() -> {
            try {
                // The saved preference/current display value may be a transport budget including
                // FEC/audio, or may predate host adaptation. Only the live encoder rate is a baseline.
                int previous = connection.refreshHostDetails().encoderBitrateKbps;
                if (previous <= 0) throw new IOException(getString(R.string.health_bitrate_unavailable));
                if ((undo && previous != expectedTrial) || (!undo && previous <= 500)) {
                    runOnUiThread(() -> {
                        if (!connected || conn != connection || isFinishing() || isAdaptiveBitrateEnabled() || adaptiveQuality != null) return;
                        currentBitrate = previous;
                        healthOriginalBitrate = healthTrialBitrate = 0;
                        if (foreground && generation == foregroundGeneration) {
                            messages.alert(undo ? R.string.health_trial_replaced : R.string.health_bitrate_minimum, Toast.LENGTH_LONG);
                        }
                    });
                    return;
                }
                if (!connected || conn != connection) return;
                int target = undo ? original : Math.max(500, (int) ((long) previous * 4 / 5));
                int applied = connection.setBitrate(target);
                runOnUiThread(() -> {
                    if (!connected || conn != connection || isFinishing() || isAdaptiveBitrateEnabled() || adaptiveQuality != null) return;
                    currentBitrate = applied;
                    healthOriginalBitrate = undo ? 0 : previous;
                    healthTrialBitrate = undo ? 0 : applied;
                    if (foreground && generation == foregroundGeneration) {
                        messages.alert(getString(R.string.stream_bitrate_applied, applied / 1000.0), Toast.LENGTH_LONG);
                    }
                });
            } catch (IOException | XmlPullParserException e) {
                showHostActionError(e, generation);
            } finally {
                runOnUiThread(() -> hostActionInProgress = false);
            }
        }, "Stream doctor bitrate trial").start();
    }

    // Nudges the host mouse back and forth on a still screen and times each nudge until its frame is shown.
    private void runLatencyTest() {
        if (latencyTestRunning || !connected) return;
        latencyTestRunning = true;
        messages.alert(R.string.latency_test_running, Toast.LENGTH_LONG);
        LatencyProbe probe = decoderRenderer.getLatencyProbe();
        NvConnection connection = conn;
        new Thread(() -> {
            final int probes = 15;
            long[] samples = new long[probes];
            int count = 0;
            short step = 8;
            long giveUpNs = System.nanoTime() + 20_000_000_000L;
            try {
                while (count < probes && connected && System.nanoTime() < giveUpNs) {
                    if (!probe.isIdle(System.nanoTime())) {
                        Thread.sleep(10);
                        continue;
                    }
                    probe.arm(System.nanoTime());
                    connection.sendMouseMove(step, (short) 0);
                    step = (short) -step;
                    long ns = probe.await(500);
                    if (ns > 0) samples[count++] = ns;
                }
            } catch (InterruptedException ignored) {
            }
            int measured = count;
            float[] result = LatencyProbe.summarize(samples, measured);
            runOnUiThread(() -> {
                latencyTestRunning = false;
                if (!foreground || !connected || isFinishing()) return;
                String message = result == null || measured < 5 ? getString(R.string.latency_test_failed) :
                        getString(R.string.latency_test_result, result[1], result[0], result[2], measured);
                LimeLog.info("Latency test: " + message);
                showStreamDialog(new NavigationDialogBuilder(this).setTitle(R.string.latency_test)
                        .setMessage(message).setPositiveButton(android.R.string.ok, null).create());
            });
        }, "Latency test").start();
    }

    private void confirmQuitApp() {
        confirmHostExit(R.string.stream_quit_game, R.string.stream_quit_game_confirm, this::quitStreamApp);
    }

    private void confirmCloseGame() {
        confirmHostExit(R.string.stream_close_game, R.string.stream_close_game_confirm, this::quitStreamApp);
    }

    private void confirmEndHostSession() {
        confirmHostExit(R.string.stream_end_session, R.string.stream_end_session_confirm, this::endHostSession);
    }

    private void confirmHostExit(int title, int message, Runnable exit) {
        androidx.appcompat.app.AlertDialog dialog = new NavigationDialogBuilder(this).setTitle(title)
                .setMessage(message).setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(title, (prompt, which) -> streamMenuAction = exit).create();
        showStreamDialog(dialog);
        // The action ends the session or the game on the PC: its button carries the same error colour as the menu item.
        android.widget.Button confirm = dialog.getButton(androidx.appcompat.app.AlertDialog.BUTTON_POSITIVE);
        if (confirm != null) {
            confirm.setTextColor(com.google.android.material.color.MaterialColors.getColor(confirm,
                    androidx.appcompat.R.attr.colorError));
        }
    }

    /** Closes the game on the PC (the native Close game, or Quit app on a host without the API) and leaves. */
    private void quitStreamApp() {
        runHostExit("Quit stream app", () -> conn.closeGame());
    }

    /** Ends the stream session on the PC and leaves; the game keeps running. */
    private void endHostSession() {
        runHostExit("End host session", () -> conn.nativeSessionAction(NativeSession.ActionId.END));
    }

    private interface HostExit {
        void run() throws IOException, XmlPullParserException;
    }

    private void runHostExit(String threadName, HostExit exit) {
        if (hostActionInProgress || !foreground || !connected) return;
        int generation = foregroundGeneration;
        hostActionInProgress = true;
        hostExitRequested = true;
        new Thread(() -> {
            try {
                exit.run();
                runOnUiThread(this::finish);
            } catch (IOException | XmlPullParserException e) {
                runOnUiThread(this::confirmHostExitFailedLater);
                showHostActionError(e, generation);
            } finally {
                runOnUiThread(() -> hostActionInProgress = false);
            }
        }, threadName).start();
    }

    /**
     * Leaves the stream as the player asked. A game set to quit on leave is quit first, while the
     * stream still holds the host connection; a failure is reported because the game stays running.
     */
    void leaveStream() {
        boolean chosen = AppQuitOnLeave.isEnabled(this, getIntent().getStringExtra(EXTRA_PC_UUID),
                app.getAppUuid(), app.getAppId());
        // A plain disconnect never waits for a host request; its callbacks check the activity.
        if (!StreamExitPolicy.quitsOnLeave(chosen, app.getRole(), connected)) {
            if (connected && conn != null) conn.announceDisconnect();
            finish();
            return;
        }
        if (hostActionInProgress) {
            messages.alert(R.string.stream_bitrate_busy, Toast.LENGTH_SHORT);
            return;
        }
        String name = appName != null ? appName : app.getAppName();
        Context context = getApplicationContext();
        hostActionInProgress = true;
        hostExitRequested = true;
        Toast.makeText(context, getString(R.string.stream_quit_on_leave_progress, name), Toast.LENGTH_SHORT).show();
        new Thread(() -> {
            Exception failure = null;
            try {
                conn.closeGame();
            } catch (IOException | XmlPullParserException e) {
                failure = e;
            }
            Exception error = failure;
            runOnUiThread(() -> {
                hostActionInProgress = false;
                // A host that ends the stream while it quits cancels the reply, so a failure counts
                // only while the stream is still up.
                if (error != null && connected) {
                    LimeLog.warning("Quit on leave failed: " + error.getMessage());
                    Toast.makeText(context, context.getString(R.string.stream_quit_on_leave_failed, name,
                            error.getMessage()), Toast.LENGTH_LONG).show();
                }
                finish();
            });
        }, "Quit on leave").start();
    }

    private void endRemoteSession() {
        if (hostActionInProgress) {
            return;
        }
        final int generation = foregroundGeneration;
        hostActionInProgress = true;
        hostExitRequested = true;
        new Thread(() -> {
            try {
                conn.disconnectRemoteSession();
                runOnUiThread(this::finish);
            } catch (IOException | XmlPullParserException e) {
                runOnUiThread(this::confirmHostExitFailedLater);
                showHostActionError(e, generation);
            } finally {
                runOnUiThread(() -> hostActionInProgress = false);
            }
        }, "End remote session").start();
    }

    private void confirmHostExitFailedLater() {
        bitrateHandler.removeCallbacks(hostExitFailed);
        bitrateHandler.postDelayed(hostExitFailed, HOST_EXIT_CONFIRM_MS);
    }

    private void refreshHostStatus() {
        if (hostActionInProgress) {
            return;
        }
        final int generation = foregroundGeneration;
        hostActionInProgress = true;
        new Thread(() -> {
            try {
                ComputerDetails details = conn.refreshHostDetails();
                runOnUiThread(() -> {
                    if (!foreground || generation != foregroundGeneration || !connected || isFinishing() || streamMenu != null) {
                        return;
                    }
                    showHostStatus(details);
                });
            } catch (IOException | XmlPullParserException e) {
                showHostActionError(e, generation);
            } finally {
                runOnUiThread(() -> hostActionInProgress = false);
            }
        }, "Host status").start();
    }

    // Creates the on-screen controller the settings ask for, or brings the existing one back.
    void showOnscreenController() {
        if (prefConfig.modernOnscreenController()) {
            if (modernController == null) {
                String game = com.limelight.binding.input.TouchLayout.profileKey(getIntent().getStringExtra(EXTRA_PC_UUID),
                        app.getAppUuid(), app.getAppId());
                modernController = new com.limelight.binding.input.pad.ModernController(this,
                        (FrameLayout) streamView.getParent(), new com.limelight.binding.input.HeldInputState.Sink() {
                            @Override public void key(int key, boolean down) { }
                            @Override public void mouse(int button, boolean down) { }
                            @Override public void gamepad(int buttons, short leftX, short leftY, short rightX, short rightY,
                                                          byte leftTrigger, byte rightTrigger) {
                                // The same on-screen gamepad report as the classic controller.
                                if (controllerHandler != null) controllerHandler.reportOscState(buttons,
                                        leftX, leftY, rightX, rightY, leftTrigger, rightTrigger);
                            }
                        }, prefConfig, game);
                modernController.setOnLayoutChanged(() -> { if (micButton != null) micButton.place(); });
                modernController.refreshLayout();
            }
            modernController.show();
            return;
        }
        if (virtualController == null) {
            virtualController = new VirtualController(controllerHandler, (FrameLayout) streamView.getParent(), this);
        }
        virtualController.refreshLayout();
        virtualController.show();
    }

    private void rememberMicrophoneMute() {
        if (conn != null && (connected || connecting)) {
            getIntent().putExtra(EXTRA_MICROPHONE_MUTED, conn.isMicrophoneMutedForReconnect());
        }
    }

    private void restoreMicrophoneMute() {
        // This is session state, applied before controller binding or native capture can start.
        conn.setMicrophoneMuted(getIntent().getBooleanExtra(EXTRA_MICROPHONE_MUTED, false));
    }

    /** The stream started again, {@code why} (a {@link LaunchSource}); the log keeps the start that opened it. */
    private Intent createStreamHandoffIntent(String why) {
        rememberMicrophoneMute();
        Intent again = LaunchSource.put(new Intent(getIntent()), LaunchSource.again(why, getIntent()));
        // The game this start replaced may be long gone: another device's could be running by now.
        again.removeExtra(EXTRA_REPLACED_APP_NAME);
        // Only a resume after the user's trip carries them; any other start spends the budget it has.
        again.removeExtra(EXTRA_TRIP_BUDGET);
        again.removeExtra(EXTRA_TRIP_RESUME_STARTED);
        return again;
    }

    private Intent createReconnectIntent() {
        Intent retry = createStreamHandoffIntent(LaunchSource.RECONNECT);
        retry.removeExtra(EXTRA_RECOVERY_ATTEMPTS);
        retry.removeExtra(EXTRA_RECOVERY_STARTED);
        retry.removeExtra(EXTRA_RECOVERY_MEDIA_FAILURE);
        retry.removeExtra(EXTRA_RECOVERY_VIDEO);
        retry.removeExtra(EXTRA_OUTPUT_RESUME_ONLY);
        if (outputSwitchPending) {
            if (app == null || app.isControlAction()) return null;
            if (app.getRole() == NvApp.Role.STREAM) {
                // Output changes only move the existing stream. Resolve Resume/Running
                // Game tiles to the actual game so a changed host session is rejected
                // by NvConnection's existing resume-only checks, never launched/quit.
                int resumeId = recoveryAppId != 0 ? recoveryAppId :
                        app.getControl() == NvApp.Control.NONE ? app.getAppId() : 0;
                if (resumeId == 0) return null;
                retry.putExtra(EXTRA_APP_ID, resumeId)
                        .putExtra(EXTRA_APP_UUID, recoveryAppId != 0 ? recoveryAppUuid : app.getAppUuid())
                        .putExtra(EXTRA_OUTPUT_RESUME_ONLY, true);
            }
            // Remote Monitor retains its dedicated view-only host operation.
        }
        return retry;
    }

    private void reconnectStream() {
        if (recoveryHandoffInProgress || isFinishing() || isDestroyed()) return;
        stoppedStreamResumeIntent = null;
        final Intent retry = createReconnectIntent();
        if (retry == null) {
            stopConnection();
            finish();
            return;
        }
        displayedFailureDialog = true;
        touchInput.setInputGrabState(false);
        stopConnection();
        retry.putExtra(EXTRA_MICROPHONE_MUTED, getIntent().getBooleanExtra(EXTRA_MICROPHONE_MUTED, false));
        final Thread stopping = connectionStopThread;
        final int generation = foregroundGeneration;
        final int recovery = recoveryGeneration;
        spinner = SpinnerDialog.displayDialog(this, getString(R.string.conn_establishing_title),
                getString(R.string.stream_reconnecting), true);
        recoveryHandoffInProgress = true;
        new Thread(() -> {
            if (stopping != null) {
                try {
                    stopping.join();
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    return;
                }
            }
            runOnUiThread(() -> {
                if (foreground && generation == foregroundGeneration && recovery == recoveryGeneration &&
                        !isFinishing() && !isDestroyed()) {
                    recoveryHandoffInProgress = false;
                    finishNow();
                    startActivity(retry);
                }
            });
        }, "Stream reconnect").start();
    }

    private boolean isRecoveryAttempt() {
        return getIntent().getIntExtra(EXTRA_RECOVERY_ATTEMPTS, 0) > 0 || isTripResume();
    }

    private boolean isResumeOnlyStream() {
        return isRecoveryAttempt() || getIntent().getBooleanExtra(EXTRA_OUTPUT_RESUME_ONLY, false);
    }

    private void cancelAutomaticRecovery() {
        recoveryGeneration++;
        recoveryHandoffInProgress = false;
        if (pendingRecovery != null) bitrateHandler.removeCallbacks(pendingRecovery);
        pendingRecovery = null;
        pendingRecoveryIntent = null;
        bitrateHandler.removeCallbacks(recoveryTimeout);
    }

    /** {@code userTrip}: the user left (Home, Files, a lock), so no connection failure caused this stop. */
    private void rememberStreamForResume(boolean userTrip) {
        if (stoppedStreamResumeIntent != null || prefConfig == null || !prefConfig.autoReconnect ||
                !connected || displayedFailureDialog || isFinishing() || isDestroyed() ||
                recoveryCancelledOnPause || recoveryHandoffInProgress || recoveryAppId == 0 ||
                app == null || app.getRole() != NvApp.Role.STREAM || app.isControlAction()) return;
        // Count only time actually connected, never time spent locked or in another app.
        long now = SystemClock.elapsedRealtime();
        recoveryPolicy.resetAfterStableConnection(now, connectedAt);
        long connectedFor = connectedAt >= 0 ? now - connectedAt : 0;
        connectedAt = -1;
        stoppedStreamResumeIntent = createStreamHandoffIntent(LaunchSource.RETURN)
                .putExtra(EXTRA_APP_ID, recoveryAppId).putExtra(EXTRA_APP_UUID, recoveryAppUuid);
        // A failure already reported as the user left is a failure: its resume spends the budget.
        if (userTrip && !streamTerminated) {
            stoppedStreamResumeIntent.putExtra(EXTRA_TRIP_BUDGET,
                    new long[] {recoveryPolicy.attempts(), recoveryPolicy.startedAt(), now, connectedFor});
        }
    }

    private boolean isTripResume() {
        return getIntent().hasExtra(EXTRA_TRIP_RESUME_STARTED);
    }

    /**
     * The failure budget this activity starts with (onCreate). A resume after the user's trip takes
     * the budget recorded as the trip began, the time since leaving not counted, so a failure while
     * it starts up continues that budget; any other start takes the one its intent carries. The
     * time connected before the trip still counts toward the 30 s that earns a new budget.
     */
    void readRecoveryBudget(long now) {
        long[] trip = getIntent().getLongArrayExtra(EXTRA_TRIP_BUDGET);
        if (trip == null) {
            recoveryPolicy = new StreamRecoveryPolicy(getIntent().getIntExtra(EXTRA_RECOVERY_ATTEMPTS, 0),
                    getIntent().getLongExtra(EXTRA_RECOVERY_STARTED, 0));
            return;
        }
        recoveryPolicy = StreamRecoveryPolicy.afterUserTrip((int) trip[0], trip[1], trip[2], now);
        connectedBeforeTrip = recoveryPolicy.attempts() > 0 ? trip[3] : 0;
        getIntent().removeExtra(EXTRA_TRIP_BUDGET);
        getIntent().putExtra(EXTRA_RECOVERY_ATTEMPTS, recoveryPolicy.attempts())
                .putExtra(EXTRA_RECOVERY_STARTED, recoveryPolicy.startedAt());
    }

    /**
     * The stream connected (connectionStarted): stability counts from here, after the time connected
     * before a trip, and the startup deadline ends.
     */
    void onStreamConnected(long now) {
        connectedAt = now - connectedBeforeTrip;
        bitrateHandler.removeCallbacks(recoveryTimeout);
    }

    // Without stream recovery, leaving the stream (Home, closing PiP) ends it, as it always did.
    // With recovery on, the activity may stay to resume the same game or to offer Reconnect.
    private boolean finishesWhenStopped() {
        return prefConfig == null || !prefConfig.autoReconnect ||
                (stoppedStreamResumeIntent == null && !recoveryCancelledOnPause);
    }

    private boolean resumeStoppedStreamIfNeeded() {
        if (stoppedStreamResumeIntent == null || !foreground || isFinishing() || isDestroyed()) return false;
        Intent retry = stoppedStreamResumeIntent;
        stoppedStreamResumeIntent = null;
        // After the user's own trip the resume skips the failure budget; after a failure it spends it.
        if (!queueAutomaticRecovery(retry, false, retry.hasExtra(EXTRA_TRIP_BUDGET))) {
            displayedFailureDialog = true;
            if (spinner != null) { spinner.dismiss(); spinner = null; }
            showReconnectDialog(R.string.conn_terminated_title, getString(R.string.stream_recovery_exhausted));
        }
        return true;
    }

    private boolean tryAutomaticRecovery() {
        return tryAutomaticRecovery(false);
    }

    /**
     * Packed full chroma stopped working: reconnect once without it for the rest of the session,
     * whether or not automatic reconnection is on (this is not a lost connection). False when this
     * session already runs without it or cannot reconnect now; the caller then ends the stream.
     */
    private boolean reconnectWithoutPackedChroma(String reason) {
        return reconnectWithoutPackedChroma("Packed full chroma stopped: " + reason, PackedFallback.toast());
    }

    private boolean reconnectWithoutPackedChroma(String logLine, String toast) {
        boolean unpacked = getIntent().getBooleanExtra(EXTRA_PACKED_OFF, false);
        boolean canReconnect = foreground && !isFinishing() && !isDestroyed();
        if (PackedFallback.onFailure(unpacked, canReconnect) != PackedFallback.Action.RECONNECT_WITHOUT_PACKED) return false;
        LimeLog.warning(logLine);
        if (!tryAutomaticRecovery(true)) return false;
        Toast.makeText(getApplicationContext(), toast, Toast.LENGTH_LONG).show();
        return true;
    }

    // Runs on the render thread: only decides on the UI thread; the stream ends from a worker if it cannot reconnect.
    private void onPackedChromaFailure(String reason) {
        runOnUiThread(() -> {
            if (isFinishing() || isDestroyed() || reconnectWithoutPackedChroma(reason)) return;
            // Whatever happens next (a queued recovery, a resume, the Reconnect dialog), copies of this
            // activity's intent keep the mark, so no later reconnect asks for packed chroma again.
            getIntent().putExtra(EXTRA_PACKED_OFF, true);
            if ((pendingRecovery != null || recoveryHandoffInProgress) && pendingRecoveryIntent != null) {
                // A recovery is already on its way (the failure came with a lost connection): its
                // replacement stream starts without packed chroma instead of a dialog over it.
                LimeLog.warning("Packed full chroma stopped during a recovery: " + reason);
                pendingRecoveryIntent.putExtra(EXTRA_PACKED_OFF, true);
                return;
            }
            if (stoppedStreamResumeIntent != null) {
                // Left in the background and kept for resuming: it resumes without packed chroma.
                LimeLog.warning("Packed full chroma stopped while the stream waits to resume: " + reason);
                stoppedStreamResumeIntent.putExtra(EXTRA_PACKED_OFF, true);
                return;
            }
            if (displayedFailureDialog) return;
            endStreamAfterPackedFailure(reason);
        });
    }

    // connectionTerminated tests the network and asks the PC (blocking calls), so never on the UI thread.
    private void endStreamAfterPackedFailure(String reason) {
        new Thread(() -> {
            MoonBridge.bridgeClDisplayMessage("Packed full-chroma presentation stopped: " + reason);
            MoonBridge.bridgeClConnectionTerminated(-1);
        }, "Packed chroma end").start();
    }

    private boolean tryAutomaticRecovery(boolean packedOff) {
        return tryAutomaticRecovery(packedOff, false);
    }

    /** {@code immediate}: no delay before the attempt (it still spends one); a network came back. */
    private boolean tryAutomaticRecovery(boolean packedOff, boolean immediate) {
        if (prefConfig == null || (!packedOff && !prefConfig.autoReconnect) || displayedFailureDialog ||
                (!connected && !connecting) || !foreground || isFinishing() || isDestroyed() ||
                app == null || app.getRole() != NvApp.Role.STREAM || app.isControlAction()) return false;
        int resumeId = recoveryAppId != 0 ? recoveryAppId :
                app.getControl() == NvApp.Control.NONE ? app.getAppId() : 0;
        String resumeUuid = recoveryAppId != 0 ? recoveryAppUuid : app.getAppUuid();
        if (resumeId == 0 && packedOff) {
            // Full chroma refused before connectionStarted resolved a Resume or Running Game tile:
            // resume the game the PC reported running, as connectionStarted would, else repeat the tile.
            ComputerDetails host = conn != null ? conn.getHostDetails() : null;
            if (host != null && host.runningGameId != 0) {
                resumeId = host.runningGameId;
                resumeUuid = host.runningGameUuid;
            } else {
                resumeId = app.getAppId();
            }
        }
        if (resumeId == 0) return false;
        Intent retry = createStreamHandoffIntent(LaunchSource.RECOVERY)
                .putExtra(EXTRA_APP_ID, resumeId).putExtra(EXTRA_APP_UUID, resumeUuid);
        if (packedOff) retry.putExtra(EXTRA_PACKED_OFF, true);
        // Only a drop after video marks the chain; a restart before it (packed chroma) carries what it had.
        if (videoStarted()) retry.putExtra(EXTRA_RECOVERY_VIDEO, true);
        return queueAutomaticRecovery(retry, packedOff, false, immediate);
    }

    /**
     * Whether video started: this connection showed a frame, or an earlier one of its automatic
     * reconnect chain did. A reconnect after a real drop that has no frame yet when it drops again
     * keeps reconnecting within the budget; a stream that never had video does not reconnect
     * (Moonlight V+ PC 6792aeed).
     */
    boolean videoStarted() {
        MediaCodecDecoderRenderer videoRenderer = decoderRenderer;
        return (videoRenderer != null && videoRenderer.hasReceivedVideo()) ||
                getIntent().getBooleanExtra(EXTRA_RECOVERY_VIDEO, false);
    }

    /**
     * Whether a lost connection would reconnect on its own now: what tryAutomaticRecovery
     * checks, and an attempt left in the budget. Spends nothing.
     */
    private boolean recoversAutomatically() {
        if (prefConfig == null || !prefConfig.autoReconnect || displayedFailureDialog || !connected || !foreground ||
                !videoStarted() ||
                isFinishing() || isDestroyed() || recoveryHandoffInProgress || pendingRecovery != null ||
                app == null || app.getRole() != NvApp.Role.STREAM || app.isControlAction()) return false;
        int resumeId = recoveryAppId != 0 ? recoveryAppId :
                app.getControl() == NvApp.Control.NONE ? app.getAppId() : 0;
        return resumeId != 0 && recoveryPolicy.allowsAttempt(SystemClock.elapsedRealtime(), connectedAt);
    }

    private boolean queueAutomaticRecovery(final Intent retry, boolean ignoreAutoReconnectSetting) {
        return queueAutomaticRecovery(retry, ignoreAutoReconnectSetting, false);
    }

    private boolean queueAutomaticRecovery(final Intent retry, boolean ignoreAutoReconnectSetting, boolean tripResume) {
        return queueAutomaticRecovery(retry, ignoreAutoReconnectSetting, tripResume, false);
    }

    /**
     * {@code tripResume}: the resume after the user's own trip. It is never counted or refused by
     * the failure budget: a fixed delay and a deadline of its own from the return.
     * {@code immediate}: no delay. Every recovery waits for a route to the PC before it starts.
     */
    private boolean queueAutomaticRecovery(final Intent retry, boolean ignoreAutoReconnectSetting, boolean tripResume,
                                           boolean immediate) {
        if (prefConfig == null || (!ignoreAutoReconnectSetting && !prefConfig.autoReconnect) || !foreground || isFinishing() || isDestroyed() ||
                recoveryHandoffInProgress || pendingRecovery != null) return false;
        long now = SystemClock.elapsedRealtime();
        long delay;
        final long deadlineFrom;
        if (tripResume) {
            delay = StreamRecoveryPolicy.TRIP_RESUME_DELAY_MS;
            deadlineFrom = now;
            retry.putExtra(EXTRA_TRIP_RESUME_STARTED, now);
        } else {
            delay = recoveryPolicy.nextDelay(now, connectedAt);
            if (delay < 0) {
                recoveryBudgetExhausted = true;
                return false;
            }
            if (immediate) delay = 0;
            deadlineFrom = recoveryPolicy.startedAt();
            retry.putExtra(EXTRA_RECOVERY_ATTEMPTS, recoveryPolicy.attempts())
                    .putExtra(EXTRA_RECOVERY_STARTED, recoveryPolicy.startedAt());
        }
        final String routeHost = getIntent().getStringExtra(EXTRA_HOST);
        final int routePort = getIntent().getIntExtra(EXTRA_PORT, NvHTTP.DEFAULT_HTTP_PORT);

        displayedFailureDialog = true;
        touchInput.setInputGrabState(false);
        stopConnection();
        if (spinner != null) spinner.dismiss();
        spinner = SpinnerDialog.displayDialog(this, getString(R.string.conn_establishing_title), tripResume ?
                getString(R.string.stream_reconnecting) :
                getString(R.string.stream_recovery_attempt, recoveryPolicy.attempts(), StreamRecoveryPolicy.MAX_ATTEMPTS), true);
        retry.putExtra(EXTRA_MICROPHONE_MUTED, getIntent().getBooleanExtra(EXTRA_MICROPHONE_MUTED, false));
        pendingRecoveryIntent = retry;
        final int generation = foregroundGeneration;
        final int recovery = recoveryGeneration;
        // Keep this true after the delayed callback hands off to its teardown worker. A
        // pause/resume in that interval must still replace the spinner with manual recovery.
        recoveryHandoffInProgress = true;
        pendingRecovery = () -> {
            pendingRecovery = null;
            if (!foreground || generation != foregroundGeneration || recovery != recoveryGeneration || isFinishing() || isDestroyed()) return;
            final Thread stopping = connectionStopThread;
            new Thread(() -> {
                try {
                    // Never overlap the native global connection with a previous stop.
                    if (stopping != null) stopping.join(10000);
                    waitForRouteToHost(routeHost, routePort, deadlineFrom + StreamRecoveryPolicy.WINDOW_MS, recovery);
                } catch (InterruptedException interrupted) {
                    Thread.currentThread().interrupt();
                    return;
                }
                runOnUiThread(() -> {
                    if (!foreground || generation != foregroundGeneration || recovery != recoveryGeneration || isFinishing() || isDestroyed()) return;
                    if ((stopping != null && stopping.isAlive()) ||
                            SystemClock.elapsedRealtime() - deadlineFrom >= StreamRecoveryPolicy.WINDOW_MS) {
                        recoveryHandoffInProgress = false;
                        if (spinner != null) { spinner.dismiss(); spinner = null; }
                        showReconnectDialog(R.string.conn_terminated_title, getString(R.string.stream_recovery_exhausted));
                        return;
                    }
                    recoveryHandoffInProgress = false;
                    finishNow();
                    startActivity(retry);
                });
            }, "Automatic stream recovery").start();
        };
        bitrateHandler.postDelayed(pendingRecovery, delay);
        return true;
    }

    /**
     * On a recovery's worker: returns once the PC is reachable, at {@code deadline}, or when the
     * recovery is cancelled. A network is often back before its address toward the PC (DHCP),
     * so the attempt waits for a route instead of failing at once and backing off.
     */
    private void waitForRouteToHost(String host, int port, long deadline, int recovery) throws InterruptedException {
        if (host == null) return;
        NetworkReturnRecovery wait = new NetworkReturnRecovery(deadline);
        long waitStarted = SystemClock.elapsedRealtime();
        while (recovery == recoveryGeneration && !isFinishing() && !isDestroyed()) {
            long checkedAt = SystemClock.elapsedRealtime();
            String unreachable = routeProbe.problem(host, port);
            long next = wait.next(SystemClock.elapsedRealtime(), unreachable == null);
            if (next == 0) {
                if (wait.unreachableChecks() > 0) {
                    LimeLog.info("Recovery: the PC is reachable again after " +
                            (SystemClock.elapsedRealtime() - waitStarted) + " ms");
                }
                return;
            }
            if (next < 0) {
                LimeLog.warning("Recovery: the PC stayed unreachable until the recovery window ended: " + unreachable);
                return;
            }
            if (wait.unreachableChecks() == 1) {
                LimeLog.info("Recovery: no route to the PC yet (" + unreachable + "), checking every " +
                        NetworkReturnRecovery.RETRY_MS + " ms");
            }
            Thread.sleep(Math.max(0, next - (SystemClock.elapsedRealtime() - checkedAt)));
        }
    }

    interface RouteProbe { String problem(String host, int port); }

    // A recovery's route check toward the PC (tests replace it).
    private RouteProbe routeProbe = Game::routeProblem;

    /**
     * Why there is no route toward the PC's HTTP port yet, or null to go on with the attempt: an
     * answer, refused included, and also a check that timed out or failed otherwise (a slow link
     * is not a missing one; the attempt itself decides, loudly).
     */
    private static String routeProblem(String host, int port) {
        try (java.net.Socket socket = new java.net.Socket()) {
            socket.connect(new java.net.InetSocketAddress(host, port), NetworkReturnRecovery.PROBE_TIMEOUT_MS);
            return null;
        } catch (java.io.IOException e) {
            return routeMissing(e) ? e.toString() : null;
        }
    }

    /** An immediate no-route error: no address toward the PC (yet), or its name does not resolve. */
    static boolean routeMissing(java.io.IOException e) {
        // Android's socket layer throws NoRouteToHostException for EHOSTUNREACH and
        // BindException ("Address not available") for EADDRNOTAVAIL, without the errno.
        if (e instanceof java.net.UnknownHostException || e instanceof java.net.NoRouteToHostException ||
                e instanceof java.net.BindException) return true;
        if (!(e.getCause() instanceof android.system.ErrnoException)) return false;
        int errno = ((android.system.ErrnoException) e.getCause()).errno;
        return errno == android.system.OsConstants.ENETUNREACH || errno == android.system.OsConstants.EHOSTUNREACH ||
                errno == android.system.OsConstants.EADDRNOTAVAIL;
    }

    private void showReconnectDialog(int title, String message) {
        showStreamDialog(new NavigationDialogBuilder(this).setTitle(title).setMessage(message)
                .setCancelable(false)
                .setNegativeButton(R.string.stream_disconnect, (dialog, which) -> finish())
                .setPositiveButton(R.string.stream_reconnect, (dialog, which) -> reconnectStream()).create());
    }

    // The devices plugged into this one that may go to the PC; picking a forwarded one detaches it.
    private void showUsbDevices() {
        if (!connected) return;
        if (usbForwarding == null) {
            usbForwarding = new com.limelight.binding.usb.UsbForwarding(this, prefConfig, conn,
                    text -> messages.alert(text, Toast.LENGTH_LONG));
        }
        com.limelight.binding.usb.UsbForwarding forwarding = usbForwarding;
        java.util.List<android.hardware.usb.UsbDevice> devices = forwarding.candidates();
        com.google.android.material.dialog.MaterialAlertDialogBuilder builder = new NavigationDialogBuilder(this)
                .setTitle(R.string.usb_forwarding_pick_title);
        if (devices.isEmpty()) {
            builder.setMessage(R.string.usb_forwarding_none).setPositiveButton(android.R.string.ok, null);
        } else {
            CharSequence[] labels = new CharSequence[devices.size()];
            for (int i = 0; i < labels.length; i++) labels[i] = forwarding.label(devices.get(i));
            builder.setItems(labels, (dialog, which) -> forwarding.choose(devices.get(which)))
                    .setNegativeButton(android.R.string.cancel, null);
        }
        showStreamDialog(builder.create());
    }

    private void transferClipboard(boolean send) {
        if (hostActionInProgress || !foreground || !connected) {
            return;
        }
        ClipboardManager clipboard = (ClipboardManager) getSystemService(CLIPBOARD_SERVICE);
        String text = null;
        if (send) {
            final ClipData clip;
            try {
                clip = clipboard.getPrimaryClip();
            } catch (RuntimeException e) {
                showHostActionError(e, foregroundGeneration);
                return;
            }
            if (clip == null || clip.getItemCount() == 0 || clip.getItemAt(0).getText() == null) {
                messages.alert(R.string.stream_clipboard_empty, Toast.LENGTH_SHORT);
                return;
            }
            text = clip.getItemAt(0).getText().toString();
        }
        final String outgoing = text;
        final int generation = foregroundGeneration;
        hostActionInProgress = true;
        new Thread(() -> {
            try {
                if (send) {
                    conn.sendClipboard(outgoing);
                }
                String incoming = send ? null : conn.getClipboard();
                runOnUiThread(() -> {
                    // Window focus is not asked: a toast or the stream menu's dialog holds it while the stream stays.
                    if (!foreground || generation != foregroundGeneration || !connected || isFinishing()) {
                        if (!send) {
                            messages.alert(R.string.stream_clipboard_not_copied, Toast.LENGTH_LONG);
                        }
                        return;
                    }
                    if (!send) {
                        ClipData clip = ClipData.newPlainText(BuildConfig.BRAND_NAME, incoming);
                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                            PersistableBundle extras = new PersistableBundle();
                            extras.putBoolean("android.content.extra.IS_SENSITIVE", true);
                            clip.getDescription().setExtras(extras);
                        }
                        try {
                            clipboard.setPrimaryClip(clip);
                        } catch (RuntimeException e) {
                            showHostActionError(e, generation);
                            return;
                        }
                    }
                    messages.alert(send ? R.string.stream_clipboard_sent : R.string.stream_clipboard_received, Toast.LENGTH_SHORT);
                });
            } catch (IOException | XmlPullParserException e) {
                showHostActionError(e, generation);
            } finally {
                runOnUiThread(() -> hostActionInProgress = false);
            }
        }, "Clipboard transfer").start();
    }

    /**
     * Sends the image on this device's clipboard to the host as a PNG, or puts the host's PNG on this device's
     * clipboard as a content URI. Android 13 and later confirm a copy themselves, so only older ones get a toast.
     */
    private void transferClipboardImage(boolean send) {
        if (hostActionInProgress || !foreground || !connected) {
            return;
        }
        ClipboardManager clipboard = (ClipboardManager) getSystemService(CLIPBOARD_SERVICE);
        ClipData current = null;
        if (send) {
            try {
                current = clipboard.getPrimaryClip();
            } catch (RuntimeException e) {
                showHostActionError(e, foregroundGeneration);
                return;
            }
            if (current == null || current.getItemCount() == 0) {
                messages.alert(R.string.stream_clipboard_image_empty, Toast.LENGTH_SHORT);
                return;
            }
        }
        final ClipData outgoing = current;
        final int generation = foregroundGeneration;
        hostActionInProgress = true;
        new Thread(() -> {
            java.io.File staged = null;
            try {
                if (send) {
                    // The image's type comes from its provider, which may block: never asked on the main thread.
                    android.net.Uri image = ClipboardImages.imageUri(outgoing, getContentResolver());
                    if (image == null) {
                        showClipboardImageResult(R.string.stream_clipboard_image_empty, generation);
                        return;
                    }
                    staged = ClipboardImages.readPng(this, image);
                    conn.sendClipboardImage(staged);
                    showClipboardImageResult(R.string.stream_clipboard_image_sent, generation);
                    return;
                }
                staged = ClipboardImages.newIncoming(this);
                boolean found;
                try (java.io.OutputStream out = new java.io.FileOutputStream(staged)) {
                    found = conn.getClipboardImage(out);
                }
                if (!found) {
                    showClipboardImageResult(R.string.stream_clipboard_image_none, generation);
                    return;
                }
                java.io.File incoming = staged;
                staged = null;
                runOnUiThread(() -> placeClipboardImage(clipboard, incoming, generation));
            } catch (ClipboardImageException e) {
                LimeLog.warning("Clipboard image refused: " + e.getMessage());
                showClipboardImageResult(ClipboardImages.message(e.reason, send), generation);
            } catch (OutOfMemoryError e) {
                LimeLog.warning("Clipboard image out of memory: " + e);
                showClipboardImageResult(ClipboardImages.message(ClipboardImageException.Reason.TOO_LARGE, send),
                        generation);
            } catch (IOException | XmlPullParserException | RuntimeException e) {
                showHostActionError(e, generation);
            } finally {
                if (staged != null) {
                    ClipboardImages.discard(staged);
                }
                runOnUiThread(() -> hostActionInProgress = false);
            }
        }, "Clipboard image transfer").start();
    }

    /**
     * Puts the host's image, streamed to {@code image}, on this device's clipboard; the images before it go only once
     * it is there. A stream that closed meanwhile gets it said, not a silent drop.
     */
    private void placeClipboardImage(ClipboardManager clipboard, java.io.File image, int generation) {
        // Window focus is not asked: a toast or the stream menu's dialog holds it while the stream stays.
        if (!foreground || generation != foregroundGeneration || !connected || isFinishing()) {
            ClipboardImages.discard(image);
            messages.alert(R.string.stream_clipboard_image_not_copied, Toast.LENGTH_LONG);
            return;
        }
        try {
            ClipData clip = ClipData.newUri(getContentResolver(), BuildConfig.BRAND_NAME, ClipboardImages.share(this, image));
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                PersistableBundle extras = new PersistableBundle();
                extras.putBoolean("android.content.extra.IS_SENSITIVE", true);
                clip.getDescription().setExtras(extras);
            }
            clipboard.setPrimaryClip(clip);
        } catch (RuntimeException e) {
            ClipboardImages.discard(image);
            showHostActionError(e, generation);
            return;
        }
        ClipboardImages.keepOnly(this, image);
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            messages.alert(R.string.stream_clipboard_image_received, Toast.LENGTH_SHORT);
        }
    }

    private void showClipboardImageResult(int message, int generation) {
        runOnUiThread(() -> {
            if (foreground && generation == foregroundGeneration && connected && !isFinishing()) {
                messages.alert(message, Toast.LENGTH_LONG);
            }
        });
    }

    private void showServerCommands(ComputerDetails details) {
        String[] names = details.serverCommands.subList(0, Math.min(256, details.serverCommands.size())).toArray(new String[0]);
        showStreamDialog(new NavigationDialogBuilder(this).setTitle(R.string.stream_server_commands)
                .setItems(names, (dialog, index) -> {
                    dialog.dismiss();
                    showStreamDialog(new NavigationDialogBuilder(this).setTitle(names[index])
                            .setMessage(R.string.stream_command_confirm)
                            .setNegativeButton(android.R.string.cancel, null)
                            .setPositiveButton(R.string.stream_command_run, (confirmation, which) -> {
                                if (!foreground || !connected || hostActionInProgress) {
                                    return;
                                }
                                hostActionInProgress = true;
                                final int generation = foregroundGeneration;
                                new Thread(() -> {
                                    try {
                                        boolean sent = conn.sendServerCommand(index, names[index]);
                                        runOnUiThread(() -> {
                                            if (foreground && generation == foregroundGeneration && connected && !isFinishing()) {
                                                messages.alert(sent ? R.string.stream_command_sent : R.string.stream_command_failed, Toast.LENGTH_LONG);
                                            }
                                        });
                                    } catch (IOException | XmlPullParserException e) {
                                        showHostActionError(e, generation);
                                    } finally {
                                        runOnUiThread(() -> hostActionInProgress = false);
                                    }
                                }, "Server command").start();
                            }).create());
                }).setNegativeButton(android.R.string.cancel, null).create());
    }

    private void showHostStatus(ComputerDetails details) {
        StringBuilder status = new StringBuilder();
        if (details.permission == -1) {
            status.append(getString(R.string.stream_permissions_unknown));
        } else {
            String[] names = getResources().getStringArray(R.array.host_permission_names);
            int[] masks = ComputerDetails.PERMISSION_DISPLAY_MASKS;
            for (int i = 0; i < names.length; i++) {
                status.append(names[i]).append(": ").append(getString(details.hasPermission(masks[i]) ?
                        R.string.stream_permission_allowed : R.string.stream_permission_denied)).append('\n');
            }
        }
        if (details.permission != -1) {
            status.append("\n\n").append(getString(R.string.stream_permissions_help));
        }
        if (details.caps.has(HostCapabilities.Feature.FRAME_LIMITER_STATUS)) {
            status.append("\n\n").append(getString(R.string.stream_limiter_status,
                    getString(details.frameLimiterEnabled ? R.string.stream_enabled : R.string.stream_disabled),
                    getString(details.virtualDisplayFrameLimiterEnabled ? R.string.stream_enabled : R.string.stream_disabled),
                    details.frameLimiterFpsLimitMilliHz == 0 ? getString(R.string.stream_limiter_stream_rate) :
                            String.format(Locale.getDefault(), "%.3f FPS", details.frameLimiterFpsLimitMilliHz / 1000.0)));
        }
        status.append("\n\n").append(getString(!prefConfig.vrr ? R.string.stream_android_pacing :
                useArr ? R.string.stream_android_arr : R.string.stream_android_max_refresh));
        showStreamDialog(new NavigationDialogBuilder(this).setTitle(R.string.stream_host_status)
                .setMessage(status).setPositiveButton(android.R.string.ok, null).create());
    }

    private void showBitrateDialog() {
        com.google.android.material.textfield.TextInputLayout field = new com.google.android.material.textfield.TextInputLayout(this);
        field.setBoxBackgroundMode(com.google.android.material.textfield.TextInputLayout.BOX_BACKGROUND_OUTLINE);
        field.setHint(getString(R.string.stream_bitrate));
        int padding = Math.round(24 * getResources().getDisplayMetrics().density);
        LinearLayout fields = new LinearLayout(this);
        fields.setOrientation(LinearLayout.VERTICAL);
        fields.setPadding(padding, 0, padding, 0);
        EditText input = new com.google.android.material.textfield.TextInputEditText(this);
        input.setMinHeight(Math.round(48 * getResources().getDisplayMetrics().density));
        input.setSingleLine(true);
        field.addView(input);
        input.setInputType(InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_FLAG_DECIMAL);
        int bitrateKbps = currentBitrate == 0 ? prefConfig.bitrate : currentBitrate;
        input.setText(BigDecimal.valueOf(bitrateKbps, 3).stripTrailingZeros().toPlainString());
        input.selectAll();
        AlertDialog bitrateDialog = new NavigationDialogBuilder(this).setTitle(R.string.stream_bitrate)
                .setMessage(R.string.stream_bitrate_help).setView(fields)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(android.R.string.ok, null).create();
        fields.addView(BitrateSlider.create(this, input, bitrateKbps, () -> applyFromSlider(bitrateDialog)));
        fields.addView(field);
        showStreamDialog(bitrateDialog);
        bitrateDialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(view -> {
            final int kbps;
            try {
                kbps = new BigDecimal(input.getText().toString().trim().replace(',', '.'))
                        .movePointRight(3).intValueExact();
                if (kbps < 500 || kbps > StreamConfiguration.MAX_BITRATE_KBPS) {
                    throw new NumberFormatException();
                }
            } catch (NumberFormatException | ArithmeticException e) {
                field.setError(getString(R.string.stream_bitrate_invalid));
                return;
            }
            if (hostActionInProgress) {
                messages.alert(R.string.stream_bitrate_busy, Toast.LENGTH_SHORT);
                return;
            }
            if (!foreground || !connected || hostActionInProgress) {
                return;
            }
            // A manual bitrate stops automatic bitrate for this session only; the saved choice stays.
            setAdaptiveBitrateEnabled(false, false);
            qualityAutoBitrate = false;
            bitrateDialog.dismiss();
            hostActionInProgress = true;
            final int generation = foregroundGeneration;
            new Thread(() -> {
                try {
                    int applied = conn.setBitrate(kbps);
                    runOnUiThread(() -> {
                        currentBitrate = applied;
                        healthOriginalBitrate = healthTrialBitrate = 0;
                        if (foreground && generation == foregroundGeneration && connected && !isFinishing()) {
                            messages.alert(getString(R.string.stream_bitrate_applied, applied / 1000.0), Toast.LENGTH_LONG);
                        }
                    });
                } catch (IOException | XmlPullParserException e) {
                    showHostActionError(e, generation);
                } finally {
                    runOnUiThread(() -> hostActionInProgress = false);
                }
            }, "Stream bitrate").start();
        });
    }

    // A settled bitrate slider sends through its dialog's OK, with the same checks as a typed value.
    private static void applyFromSlider(AlertDialog dialog) {
        if (dialog != null && dialog.isShowing()) dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
    }

    // Bitrate and frame rate in one 0x553C request; a field left as it is keeps its value.
    private void showRateChangeDialog() {
        final StreamRateChanges stream = rateChanges;
        if (stream == null) return;
        float density = getResources().getDisplayMetrics().density;
        int padding = Math.round(24 * density);
        LinearLayout fields = new LinearLayout(this);
        fields.setOrientation(LinearLayout.VERTICAL);
        fields.setPadding(padding, 0, padding, 0);
        int bitrateKbps = currentBitrate == 0 ? prefConfig.bitrate : currentBitrate;
        String bitrateText = BigDecimal.valueOf(bitrateKbps, 3).stripTrailingZeros().toPlainString();
        com.google.android.material.textfield.TextInputLayout bitrateField = rateChangeField(
                getString(R.string.stream_bitrate), bitrateText, density);
        final AlertDialog[] opened = new AlertDialog[1];
        fields.addView(BitrateSlider.create(this, bitrateField.getEditText(), bitrateKbps,
                () -> applyFromSlider(opened[0])));
        fields.addView(bitrateField);
        // Automatic quality owns the frame rate while it runs.
        int fpsMillihz = reconfigureRequester.fpsMillihz() > 0 ? reconfigureRequester.fpsMillihz() : streamFps * 1000;
        String fpsText = BigDecimal.valueOf(fpsMillihz, 3).stripTrailingZeros().toPlainString();
        com.google.android.material.textfield.TextInputLayout fpsField = adaptiveQuality != null ? null :
                rateChangeField(getString(R.string.stream_rate_change_fps), fpsText, density);
        if (fpsField != null) fields.addView(fpsField);
        AlertDialog dialog = new NavigationDialogBuilder(this).setTitle(R.string.stream_rate_change)
                .setMessage(R.string.stream_rate_change_help).setView(fields)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(android.R.string.ok, null).create();
        opened[0] = dialog;
        showStreamDialog(dialog);
        dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(view -> {
            // connected stays true until stopConnection, which closes this dialog: a termination
            // that has arrived but not yet stopped the stream counts as ended (streamEnding).
            if (!connected || streamEnding() || rateChanges != stream) {
                // The stream this dialog was opened for ended or restarted: nothing goes out.
                dialog.dismiss();
                LimeLog.warning("Rate change: the stream ended before the request was sent");
                messages.alert(R.string.stream_rate_change_stream_ended, Toast.LENGTH_LONG);
                return;
            }
            bitrateField.setError(null);
            if (fpsField != null) fpsField.setError(null);
            int kbps = parseRateChange(bitrateField, bitrateText, 500, StreamConfiguration.MAX_BITRATE_KBPS,
                    R.string.stream_bitrate_invalid);
            int millihz = fpsField == null ? 0 : parseRateChange(fpsField, fpsText, 10_000, 500_000,
                    R.string.stream_rate_change_fps_invalid);
            if (kbps < 0 || millihz < 0) return;
            if (kbps == 0 && millihz == 0) {
                bitrateField.setError(getString(R.string.stream_rate_change_nothing));
                return;
            }
            dialog.dismiss();
            if (!stream.sendManual(kbps, millihz, SystemClock.uptimeMillis())) {
                LimeLog.warning("Rate change: request not sent");
                messages.alert(R.string.stream_rate_change_not_sent, Toast.LENGTH_LONG);
                return;
            }
            scheduleRateChangeExpiry();
            LimeLog.info("Rate change: asking for " + kbps + " kbps, " + millihz + " mHz");
            if (kbps != 0) {
                // As with the bitrate dialog, a bitrate set by hand stops the host-side automatic
                // bitrate for this session; automatic bitrate on this device follows it instead.
                setAdaptiveBitrateEnabled(false, false);
                qualityAutoBitrate = false;
                healthOriginalBitrate = healthTrialBitrate = 0;
            }
        });
    }

    private com.google.android.material.textfield.TextInputLayout rateChangeField(String hint, String text, float density) {
        com.google.android.material.textfield.TextInputLayout field = new com.google.android.material.textfield.TextInputLayout(this);
        field.setBoxBackgroundMode(com.google.android.material.textfield.TextInputLayout.BOX_BACKGROUND_OUTLINE);
        field.setHint(hint);
        EditText input = new com.google.android.material.textfield.TextInputEditText(this);
        input.setMinHeight(Math.round(48 * density));
        input.setSingleLine(true);
        input.setInputType(InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_FLAG_DECIMAL);
        input.setText(text);
        field.addView(input);
        return field;
    }

    // The field's value in thousandths (kbps from Mbps, mHz from fps); 0 when it is empty or as it
    // was, -1 with the error shown when it is outside min-max.
    private int parseRateChange(com.google.android.material.textfield.TextInputLayout field, String unchanged,
                                int min, int max, int error) {
        String text = field.getEditText().getText().toString().trim().replace(',', '.');
        if (text.isEmpty() || text.equals(unchanged)) return 0;
        try {
            int value = new BigDecimal(text).movePointRight(3).intValueExact();
            if (value >= min && value <= max) return value;
        } catch (NumberFormatException | ArithmeticException e) {
            // Shown below.
        }
        field.setError(getString(error));
        return -1;
    }

    // The host's answer to a 0x553C request, on the UI thread.
    private void onRateChange(RateChangeResult result) {
        if (rateChanges == null || !connected || isFinishing()) return;
        StreamRateChanges.Outcome outcome = rateChanges.onResult(result, SystemClock.uptimeMillis());
        StreamRateChanges.Answer answer = outcome.answer;
        if (answer == StreamRateChanges.Answer.UNKNOWN) {
            LimeLog.warning("Rate change: a result for no request of this stream, " + result);
            return;
        }
        LimeLog.info("Rate change: " + result + " (" + answer + (outcome.takesBitrate ? ")" : ", older bitrate)"));
        // An answer that came after a later request's keeps that one's bitrate.
        if (outcome.takesBitrate) currentBitrate = result.bitrateKbps;
        if (result.fpsChanged() && result.fpsMillihz > 0 && result.fpsMillihz != reconfigureRequester.fpsMillihz()) {
            reconfigureRequester.rateChanged(result.fpsMillihz);
            setStreamFrameRate(result.fpsMillihz);
        }
        if (answer == StreamRateChanges.Answer.AUTOMATIC_STOPPED) {
            bitrateHandler.removeCallbacks(updateRateChangeBitrate);
            LimeLog.warning("Automatic bitrate: the host refused a rate change; off for this stream");
            messages.alert(getString(R.string.stream_rate_change_auto_stopped,
                    rateChangeReason(this, result.bitrateReason)), Toast.LENGTH_LONG);
        } else if (answer == StreamRateChanges.Answer.MANUAL) {
            messages.alert(rateChangeText(this, result), Toast.LENGTH_LONG);
        }
    }

    // What became of each value asked for, in plain words.
    static String rateChangeText(Context context, RateChangeResult result) {
        StringBuilder text = new StringBuilder();
        double mbps = result.bitrateKbps / 1000.0;
        switch (result.bitrateStatus) {
            case RateChangeResult.STATUS_APPLIED:
                text.append(context.getString(R.string.stream_rate_change_bitrate_applied, mbps));
                break;
            case RateChangeResult.STATUS_CLAMPED:
                text.append(context.getString(R.string.stream_rate_change_bitrate_clamped, mbps,
                        rateChangeReason(context, result.bitrateReason)));
                break;
            case RateChangeResult.STATUS_REFUSED:
                text.append(context.getString(R.string.stream_rate_change_bitrate_refused, mbps,
                        rateChangeReason(context, result.bitrateReason)));
                break;
            default:
                break;
        }
        String fps = BigDecimal.valueOf(result.fpsMillihz, 3).stripTrailingZeros().toPlainString();
        int fpsText = result.fpsStatus == RateChangeResult.STATUS_APPLIED ? R.string.stream_rate_change_fps_applied :
                result.fpsStatus == RateChangeResult.STATUS_CLAMPED ? R.string.stream_rate_change_fps_clamped :
                result.fpsStatus == RateChangeResult.STATUS_REFUSED ? R.string.stream_rate_change_fps_refused : 0;
        if (fpsText != 0) {
            if (text.length() > 0) text.append('\n');
            text.append(fpsText == R.string.stream_rate_change_fps_applied ? context.getString(fpsText, fps) :
                    context.getString(fpsText, fps, rateChangeReason(context, result.fpsReason)));
        }
        return text.toString();
    }

    static String rateChangeReason(Context context, int reason) {
        switch (reason) {
            case RateChangeResult.REASON_INVALID: return context.getString(R.string.stream_rate_change_reason_invalid);
            case RateChangeResult.REASON_DISABLED: return context.getString(R.string.stream_rate_change_reason_disabled);
            case RateChangeResult.REASON_CODEC: return context.getString(R.string.stream_rate_change_reason_codec);
            case RateChangeResult.REASON_HOST_LIMIT: return context.getString(R.string.stream_rate_change_reason_host_limit);
            case RateChangeResult.REASON_ENCODER_LIMIT: return context.getString(R.string.stream_rate_change_reason_encoder_limit);
            case RateChangeResult.REASON_ENCODER_FAILED: return context.getString(R.string.stream_rate_change_reason_encoder_failed);
            case RateChangeResult.REASON_SUPERSEDED: return context.getString(R.string.stream_rate_change_reason_superseded);
            default: return context.getString(R.string.stream_rate_change_reason_other);
        }
    }

    // At the stream's start: rate changes when the host offered them, and the automatic bitrate
    // the settings ask for, on this device or else on the host.
    private void startStreamBitrate() {
        startStreamBitrate(offersRateChange() ? new StreamRateChanges(MoonBridge::sendRateChange) : null);
    }

    private void startStreamBitrate(StreamRateChanges changes) {
        rateChanges = changes;
        if (rateChanges != null && prefConfig.rateChangeBitrate) {
            setRateChangeBitrateEnabled(true);
        } else if (autoBitrateAtStart()) {
            setAdaptiveBitrateEnabled(true, false);
        } else if (foundationAbrAtStart()) {
            startFoundationAbr();
        }
    }

    // Automatic bitrate on a PC without Rubylight's runtime bitrate: a Foundation Sunshine PC runs
    // its own controller (as Moonlight V+ asks it to); other PCs answer 404 and nothing changes.
    private boolean foundationAbrAtStart() {
        ComputerDetails details = conn.getHostDetails();
        return connected && app != null && app.getRole() == NvApp.Role.STREAM &&
                details.caps.family() != HostCapabilities.Family.RUBYLIGHT &&
                !details.caps.has(HostCapabilities.Feature.RUNTIME_BITRATE) &&
                AutoBitrateChoice.isOn(this, adaptiveBitratePreferences(), getIntent().getStringExtra(EXTRA_PC_UUID));
    }

    private void startFoundationAbr() {
        NvConnection connection = conn;
        int maxKbps = currentBitrate == 0 ? prefConfig.bitrate : currentBitrate;
        new Thread(() -> {
            try {
                int initial = connection.startFoundationAbr(maxKbps);
                runOnUiThread(() -> {
                    if (initial < 0) {
                        LimeLog.info("The PC has no automatic bitrate");
                        return;
                    }
                    if (!connected || isFinishing() || conn != connection) {
                        // The stream ended while the PC turned it on: stopFoundationAbr had nothing to stop.
                        sendFoundationAbrStop(connection);
                        return;
                    }
                    foundationAbr = new com.limelight.nvstream.http.FoundationAbr(initial, maxKbps, SystemClock.uptimeMillis());
                    currentBitrate = foundationAbr.currentKbps();
                    LimeLog.info("The PC's automatic bitrate is on: " + currentBitrate + " kbps, at most " + maxKbps + " kbps");
                    bitrateHandler.postDelayed(foundationAbrTick, 1000);
                });
            } catch (IOException e) {
                LimeLog.warning("The PC did not start its automatic bitrate: " + e.getMessage());
                // Loud, as on desktop: the stream keeps its bitrate and the user should know why.
                runOnUiThread(() -> {
                    if (connected && !isFinishing() && conn == connection) {
                        messages.alert(getString(R.string.stream_host_abr_failed, e.getMessage()), Toast.LENGTH_LONG);
                    }
                });
            }
        }, "FoundationAbr").start();
    }

    // Turns a Foundation PC's controller off at the stream's end (V+ PC's stopSunshineAbr).
    private void stopFoundationAbr() {
        if (foundationAbr == null) return;
        foundationAbr = null;
        bitrateHandler.removeCallbacks(foundationAbrTick);
        sendFoundationAbrStop(conn);
    }

    private static void sendFoundationAbrStop(NvConnection connection) {
        new Thread(() -> {
            try {
                connection.stopFoundationAbr();
            } catch (IOException e) {
                LimeLog.warning("The PC's automatic bitrate did not stop: " + e.getMessage());
            }
        }, "FoundationAbr").start();
    }

    // The host offered this stream 0x553C, which only the Rust engine asks for (rlRateChange).
    private boolean offersRateChange() {
        return connected && app != null && app.getRole() == NvApp.Role.STREAM && MoonBridge.isRustStream() &&
                MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_RATE_CHANGE);
    }

    // The next check for an unanswered 0x553C request, when one is outstanding.
    private void scheduleRateChangeExpiry() {
        bitrateHandler.removeCallbacks(expireRateChanges);
        long at = rateChanges == null ? 0 : rateChanges.nextExpiryMs();
        if (at > 0) bitrateHandler.postAtTime(expireRateChanges, at);
    }

    private boolean isRateChangeBitrateOn() {
        return rateChanges != null && rateChanges.automaticOn();
    }

    // Automatic bitrate on this device, for this stream; the host-side automatic bitrate stops,
    // since both would set the same encoder.
    private void setRateChangeBitrateEnabled(boolean enabled) {
        bitrateHandler.removeCallbacks(updateRateChangeBitrate);
        if (rateChanges == null) return;
        if (!enabled) {
            rateChanges.stopAutomatic();
            return;
        }
        if (isAdaptiveBitrateEnabled()) setAdaptiveBitrateEnabled(false, false);
        qualityAutoBitrate = false;
        healthOriginalBitrate = healthTrialBitrate = 0;
        rateChanges.startAutomatic(currentBitrate == 0 ? prefConfig.bitrate : currentBitrate,
                StreamRateChanges.clientLimitKbps(prefConfig.layers == null ? null : prefConfig.layers.network,
                        prefConfig.mobileBitrateLimit),
                SystemClock.uptimeMillis());
        bitrateHandler.postDelayed(updateRateChangeBitrate, 1000);
    }

    private boolean supportsAdaptiveBitrate() {
        ComputerDetails details = conn.getHostDetails();
        return app.getRole() != NvApp.Role.INPUT_ONLY && details.caps.has(HostCapabilities.Feature.RUNTIME_BITRATE) &&
                details.encoderBitrateSupported &&
                details.hasPermission(ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_LAUNCH);
    }

    AdaptiveQualityController.Mode qualityMode() {
        return AdaptiveQualityController.Mode.parse(getSharedPreferences("AdaptiveQuality", MODE_PRIVATE)
                .getString(getIntent().getStringExtra(EXTRA_PC_UUID), prefConfig.adaptiveQualityMode.preference()));
    }

    private boolean supportsAdaptiveQuality() {
        return connected && decoderRenderer != null && app != null && app.getRole() == NvApp.Role.STREAM &&
                (decoderRenderer.getActiveVideoFormat() & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) == 0 &&
                MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_RECONFIGURE) &&
                decoderRenderer.getReconfigureLimits() != null;
    }

    private void startAdaptiveQuality() {
        bitrateHandler.removeCallbacks(updateQuality);
        adaptiveQuality = null;
        pendingQualityTarget = null;
        qualityStatus = null;
        if (decoderRenderer != null) decoderRenderer.resetDecodeTiming();
        AdaptiveQualityController.Mode mode = qualityMode();
        if (mode == AdaptiveQualityController.Mode.OFF) {
            releaseQualityAutoBitrate();
            return;
        }
        String pcUuid = getIntent().getStringExtra(EXTRA_PC_UUID);
        if (!supportsAdaptiveQuality()) {
            releaseQualityAutoBitrate();
            qualityStatus = getString(R.string.quality_unavailable);
            // Said once per PC; the performance overlay keeps showing it.
            if (foreground) messages.notice(pcUuid, HostNotices.Notice.AUTOMATIC_QUALITY_UNAVAILABLE, qualityStatus);
            return;
        }
        HostNotices.reset(this, pcUuid, HostNotices.Notice.AUTOMATIC_QUALITY_UNAVAILABLE);
        healthOriginalBitrate = healthTrialBitrate = 0;
        int[] limits = decoderRenderer.getReconfigureLimits();
        adaptiveQuality = new AdaptiveQualityController(mode, qualityMaxWidth, qualityMaxHeight, qualityMaxFps,
                prefConfig.qualityMinShortEdge, prefConfig.qualityMinFps, limits[2], limits[3], SystemClock.uptimeMillis());
        // A user changing the mode returns to the session's ceiling before the new policy starts.
        int[] shown = decoderRenderer.getStreamSize();
        if ((!adaptiveQuality.current().matches(shown[0], shown[1], streamFps) ||
                reconfigureRequester.fpsMillihz() != qualityCeilingFrameRateMilliHz()) &&
                !applyAdaptiveQuality(adaptiveQuality.current(), SystemClock.uptimeMillis())) return;
        // Automatic quality needs it for this session only; the saved choice stays the user's own.
        if (supportsAdaptiveBitrate() && !isAdaptiveBitrateEnabled() && !isRateChangeBitrateOn()) {
            setAdaptiveBitrateEnabled(true, false);
            qualityAutoBitrate = true;
        }
        bitrateHandler.postDelayed(updateQuality, 1000);
    }

    private boolean applyAdaptiveQuality(AdaptiveQualityController.Target target, long now) {
        return applyAdaptiveQuality(target, now, qualityCeilingFrameRateMilliHz());
    }

    private int qualityCeilingFrameRateMilliHz() {
        return qualityMaxFrameRateMilliHz > 0 ? qualityMaxFrameRateMilliHz : qualityMaxFps * 1000;
    }

    private boolean applyAdaptiveQuality(AdaptiveQualityController.Target target, long now, int ceilingMilliHz) {
        int[] request = target.request();
        if (target.fps == Math.round(ceilingMilliHz / 1000f)) request[2] = ceilingMilliHz;
        if (!requestStreamReconfigure(request)) {
            stopAdaptiveQualityWithError();
            return false;
        }
        healthOriginalBitrate = healthTrialBitrate = 0;
        adaptiveQuality.applied(target, now);
        pendingQualityTarget = target;
        qualityRequestSentAt = now;
        return true;
    }

    private void stopAdaptiveQualityWithError() {
        adaptiveQuality = null;
        pendingQualityTarget = null;
        bitrateHandler.removeCallbacks(updateQuality);
        releaseQualityAutoBitrate();
        qualityStatus = getString(R.string.quality_request_failed);
        LimeLog.warning("Automatic quality: requested size was not confirmed; adaptation stopped");
        if (foreground) messages.alert(qualityStatus, Toast.LENGTH_LONG);
    }

    private void showAdaptiveQualityDialog() {
        String[] modes = getResources().getStringArray(R.array.adaptive_quality_names);
        String[] choices = new String[] {getString(R.string.quality_use_settings), modes[0], modes[1], modes[2], modes[3]};
        SharedPreferences saved = getSharedPreferences("AdaptiveQuality", MODE_PRIVATE);
        String key = getIntent().getStringExtra(EXTRA_PC_UUID);
        int selected = saved.contains(key) ? qualityMode().ordinal() + 1 : 0;
        showStreamDialog(new NavigationDialogBuilder(this).setTitle(R.string.title_adaptive_quality)
                .setSingleChoiceItems(choices, selected, (dialog, which) -> {
                    if (!foreground || !connected || hostActionInProgress) return;
                    if (which == 0) saved.edit().remove(key).apply();
                    else saved.edit().putString(key, AdaptiveQualityController.Mode.values()[which - 1].preference()).apply();
                    dialog.dismiss();
                    startAdaptiveQuality();
                    if (qualityMode() == AdaptiveQualityController.Mode.OFF && supportsAdaptiveQuality()) {
                        requestStreamReconfigure(new int[] {qualityMaxWidth, qualityMaxHeight, qualityCeilingFrameRateMilliHz()});
                    }
                })
                .setNeutralButton(R.string.quality_about, (dialog, which) ->
                        showStreamDialog(new NavigationDialogBuilder(this).setTitle(R.string.title_adaptive_quality)
                                .setMessage(getString(R.string.quality_stream_help) + "\n\n" +
                                        getString(supportsAdaptiveQuality() ? R.string.summary_adaptive_quality : R.string.quality_unavailable))
                                .setPositiveButton(android.R.string.ok, null).create()))
                .setNegativeButton(android.R.string.cancel, null).create());
    }

    // Automatic quality is off now: auto bitrate goes back to the person's choice, which was off.
    private void releaseQualityAutoBitrate() {
        if (!qualityAutoBitrate) return;
        qualityAutoBitrate = false;
        if (isAdaptiveBitrateEnabled()) setAdaptiveBitrateEnabled(false, false);
    }

    boolean isAdaptiveBitrateEnabled() {
        return adaptiveBitrate != null || pyroWaveBitrate != null;
    }

    private String adaptiveBitratePreferences() {
        return (decoderRenderer.getActiveVideoFormat() & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0 ?
                AutoBitrateChoice.PYROWAVE_STORE : AutoBitrateChoice.CODEC_STORE;
    }

    /** Whether the stream starts with automatic bitrate: this PC's choice, else the Settings switch. */
    boolean autoBitrateAtStart() {
        return supportsAdaptiveBitrate() &&
                AutoBitrateChoice.isOn(this, adaptiveBitratePreferences(), getIntent().getStringExtra(EXTRA_PC_UUID));
    }

    private void setAdaptiveBitrateEnabled(boolean enabled, boolean remember) {
        if (remember) {
            AutoBitrateChoice.setForPc(this, adaptiveBitratePreferences(),
                    getIntent().getStringExtra(EXTRA_PC_UUID), enabled);
            // The person chose it themselves; Automatic quality no longer owns it.
            qualityAutoBitrate = false;
        }
        bitrateHandler.removeCallbacks(updateBitrate);
        adaptiveBitrate = null;
        pyroWaveBitrate = null;
        adaptiveBitrateRetryAt = 0;
        adaptiveBitrateClaimFailures = 0;
        if (enabled) {
            healthOriginalBitrate = healthTrialBitrate = 0;
            if ((decoderRenderer.getActiveVideoFormat() & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
                pyroWaveBitrate = new PyroWaveBitrateController(0, 0, streamFps,
                        SystemClock.uptimeMillis());
            } else {
                adaptiveBitrate = new AdaptiveBitrateController(0, 0, streamFps, SystemClock.uptimeMillis(),
                        prefConfig.width, prefConfig.height, adaptiveBitrateCodec());
            }
            // The preference includes FEC/audio, while runtime changes use encoder Kbps.
            // Read the actual encoder budget, then claim ownership at that same bitrate.
            applyAdaptiveBitrate(0);
            bitrateHandler.postDelayed(updateBitrate, 1000);
        }
    }

    // The codec sets how many bits a picture of this size needs, which the built-in floor follows.
    private AdaptiveBitrateController.Codec adaptiveBitrateCodec() {
        if (decoderRenderer == null) return null;
        int format = decoderRenderer.getActiveVideoFormat();
        if ((format & MoonBridge.VIDEO_FORMAT_MASK_AV1) != 0) return AdaptiveBitrateController.Codec.AV1;
        if ((format & MoonBridge.VIDEO_FORMAT_MASK_H265) != 0) return AdaptiveBitrateController.Codec.HEVC;
        return AdaptiveBitrateController.Codec.H264;
    }

    private void applyAdaptiveBitrate(int target) {
        AdaptiveBitrateController controller = adaptiveBitrate;
        PyroWaveBitrateController pyroController = pyroWaveBitrate;
        hostActionInProgress = true;
        new Thread(() -> {
            try {
                ComputerDetails details = target == 0 ? conn.refreshHostDetails() : null;
                if (details != null && (details.encoderBitrateKbps <= 0 || details.initialEncoderBitrateKbps <= 0)) {
                    // A host without the bitrate fields will not start sending them on a retry.
                    throw details.encoderBitrateSupported ?
                            new IOException("Host did not report the stream's encoder bitrate") :
                            new EncoderBitrateUnsupported();
                }
                // Claim bitrate ownership on hosts with their own controller, without probing a new rate.
                int applied = conn.setBitrate(details == null ? target : details.encoderBitrateKbps);
                runOnUiThread(() -> {
                    if (connected && !isFinishing()) {
                        if (adaptiveBitrate == controller && pyroWaveBitrate == pyroController) {
                            currentBitrate = applied;
                            adaptiveBitrateRetryAt = 0;
                            adaptiveBitrateClaimFailures = 0;
                            long now = SystemClock.uptimeMillis();
                            if (details != null) {
                                // The bitrate the stream started at (the bitrate setting, after profiles) is the
                                // ceiling; the built-in floor applies.
                                int ceiling = Math.max(details.initialEncoderBitrateKbps, applied);
                                if (controller != null) adaptiveBitrate = new AdaptiveBitrateController(ceiling, applied, streamFps, now,
                                        prefConfig.width, prefConfig.height, adaptiveBitrateCodec());
                                if (pyroController != null) pyroWaveBitrate = new PyroWaveBitrateController(ceiling, applied, streamFps, now);
                            } else {
                                if (controller != null) controller.applied(target, applied, now);
                                if (pyroController != null) pyroController.applied(target, applied, now);
                            }
                        }
                    }
                });
            } catch (IOException | XmlPullParserException e) {
                LimeLog.warning("Automatic bitrate update failed: " + e);
                runOnUiThread(() -> {
                    if (isFinishing() || isDestroyed()) return;
                    if (adaptiveBitrate == controller && pyroWaveBitrate == pyroController) {
                        if (target == 0 && (e instanceof EncoderBitrateUnsupported ||
                                ++adaptiveBitrateClaimFailures >= MAX_BITRATE_CLAIM_FAILURES)) {
                            // Without the encoder's bitrate there is nothing to adapt from: stop
                            // retrying for this stream and leave the PC's saved choice alone.
                            LimeLog.warning("Automatic bitrate: the host reports no encoder bitrate; off for this stream");
                            qualityAutoBitrate = false;
                            setAdaptiveBitrateEnabled(false, false);
                            if (foreground && connected && !isFinishing()) {
                                messages.alert(R.string.stream_auto_bitrate_unavailable, Toast.LENGTH_LONG);
                            }
                            return;
                        }
                        boolean firstFailure = adaptiveBitrateRetryAt == 0;
                        adaptiveBitrateRetryTarget = target;
                        adaptiveBitrateRetryAt = SystemClock.uptimeMillis() + 5000;
                        if (controller != null) controller.suspend();
                        if (pyroController != null) pyroController.suspend();
                        if (firstFailure && foreground && connected && !isFinishing()) {
                            messages.alert(R.string.stream_auto_bitrate_failed, Toast.LENGTH_LONG);
                        }
                    }
                });
            } finally {
                runOnUiThread(() -> hostActionInProgress = false);
            }
        }, "Automatic bitrate").start();
    }

    private static final class EncoderBitrateUnsupported extends IOException {
        EncoderBitrateUnsupported() {
            super("Host does not support runtime bitrate changes");
        }
    }

    private void showHostActionError(Exception error, int generation) {
        runOnUiThread(() -> {
            if (foreground && generation == foregroundGeneration && connected && !isFinishing()) {
                messages.alert(getString(R.string.stream_host_action_failed, error.getMessage()), Toast.LENGTH_LONG);
            }
        });
    }

    final Runnable toggleGrab = new Runnable() {
        @Override
        public void run() {
            touchInput.setInputGrabState(!grabbedInput);
        }
    };

    @Override
    public boolean onKeyDown(int keyCode, KeyEvent event) {
        return handleKeyDown(event) || super.onKeyDown(keyCode, event);
    }

    @Override
    public boolean handleKeyDown(KeyEvent event) {
        return keyboardInput.handleKeyDown(event);
    }

    @Override
    public boolean onKeyUp(int keyCode, KeyEvent event) {
        return handleKeyUp(event) || super.onKeyUp(keyCode, event);
    }

    @Override
    public boolean handleKeyUp(KeyEvent event) {
        return keyboardInput.handleKeyUp(event);
    }

    @Override
    public boolean onKeyMultiple(int keyCode, int repeatCount, KeyEvent event) {
        return keyboardInput.handleKeyMultiple(event) || super.onKeyMultiple(keyCode, repeatCount, event);
    }

    @Override
    public boolean handleTextInput(CharSequence text) {
        return keyboardInput.handleTextInput(text);
    }

    @Override
    public void toggleKeyboard() {
        keyboardInput.toggleKeyboard();
    }

    @Override
    public boolean onGenericMotionEvent(MotionEvent event) {
        return touchInput.handleMotionEvent(null, event) || super.onGenericMotionEvent(event);

    }

    @Override
    public boolean onGenericMotion(View view, MotionEvent event) {
        return touchInput.handleMotionEvent(view, event);
    }

    @SuppressLint("ClickableViewAccessibility")
    @Override
    public boolean onTouch(View view, MotionEvent event) {
        return touchInput.onTouch(view, event);
    }

    @Override
    public void stageStarting(final String stage) {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (spinner != null) {
                    spinner.setMessage(getResources().getString(R.string.conn_starting) + " " + stage);
                }
            }
        });
    }

    @Override
    public void stageComplete(String stage) {
    }

    private void stopConnection() {
        stopFoundationAbr();
        touchInput.releaseDesktopInput();
        if (trackpadPointer != null) trackpadPointer.stopMomentum();
        rememberMicrophoneMute();
        cancelAutomaticRecovery();
        if (gameTouchControls != null) gameTouchControls.releaseInput();
        overlays.stopHardwareMetricsPolling();
        getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        if (lowLatencyWifiLock != null && lowLatencyWifiLock.isHeld()) {
            lowLatencyWifiLock.release();
        }
        if (highPerfWifiLock != null && highPerfWifiLock.isHeld()) {
            highPerfWifiLock.release();
        }
        streamMenuAction = null;
        windowFocusActions.clear();
        getWindow().getDecorView().removeCallbacks(runWindowFocusActions);
        bitrateHandler.removeCallbacks(updateBitrate);
        bitrateHandler.removeCallbacks(updateRateChangeBitrate);
        bitrateHandler.removeCallbacks(expireRateChanges);
        rateChanges = null;
        bitrateHandler.removeCallbacks(checkReconfigure);
        bitrateHandler.removeCallbacks(updateQuality);
        adaptiveQuality = null;
        pendingQualityTarget = null;
        adaptiveBitrate = null;
        qualityAutoBitrate = false;
        bitrateHandler.removeCallbacks(sampleStreamHealth);
        bitrateHandler.removeCallbacks(pollContentRateVote);
        stopRefreshSwitchPolling();
        if (wifiDiagnostics != null) wifiDiagnostics.stop();
        pyroWaveBitrate = null;
        displayCapsReporter = null;
        touchInput.stopHostCursorShapes();
        if (clipboardSync != null) {
            clipboardSync.stop();
            clipboardSync = null;
        }
        // Before the connection stops: detaching tells the host.
        if (usbForwarding != null) {
            usbForwarding.stop();
            usbForwarding = null;
        }
        if (usbDriverBinder != null) {
            usbDriverBinder.setListener(null);
            usbDriverBinder.setStateListener(null);
            usbDriverBinder.stop();
            usbDriverBinder = null;
        }
        if (connecting || connected) {
            connecting = connected = false;
            updatePipAutoEnter();

            if (controllerHandler != null) controllerHandler.stop();

            // USB teardown can report a final host mute before releasing its route.
            // Keep that mute after the native connection clears controller state.
            if (conn.isMicrophoneMutedForReconnect()) getIntent().putExtra(EXTRA_MICROPHONE_MUTED, true);

            // Update GameManager state to indicate we're no longer in game
            UiHelper.notifyStreamEnded(this);

            // Stop may take a few hundred ms to do some network I/O to tell
            // the server we're going away and clean up. Let it run in a separate
            // thread to keep things smooth for the UI. Inside moonlight-common,
            // we prevent another thread from starting a connection before and
            // during the process of stopping this one.
            connectionStopThread = new Thread(conn::stop, "Stop stream");
            connectionStopThread.start();
            LimeLog.info("Stream stopping");
        }
    }

    @Override
    public void stageFailed(final String stage, final int portFlags, final int errorCode) {
        LimeLog.info("Stream failed");
        // Perform a connection test if the failure could be due to a blocked port
        // This does network I/O, so don't do it on the main thread.
        // Experimental full chroma refused by the PC (or no mode in common): not a network failure.
        final boolean packedRefused = errorCode == MoonBridge.CLIENT_ERROR_PACKED_444_REFUSED;
        final boolean transientRecoveryFailure = !packedRefused && isRecoveryAttempt() &&
                StreamRecoveryPolicy.transientStartup(stage, portFlags, errorCode);
        final int portTestResult = portFlags == 0 || transientRecoveryFailure || packedRefused ?
                MoonBridge.ML_TEST_RESULT_INCONCLUSIVE :
                MoonBridge.testClientConnectivity(ServerHelper.CONNECTION_TEST_SERVER, 443, portFlags);

        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (isFinishing() || isDestroyed()) {
                    return;
                }
                // Once per session: the replacement starts without full chroma. A refusal in a session
                // already without it (never expected: it does not ask) shows the error as usual.
                if (packedRefused && reconnectWithoutPackedChroma("Full chroma refused at stream start (" +
                        terminationDescription(errorCode) + (connectionErrorMessage != null ?
                        "): " + connectionErrorMessage : ")"), PackedFallback.refusedToast())) return;
                if (transientRecoveryFailure && tryAutomaticRecovery()) return;
                if (spinner != null) {
                    spinner.dismiss();
                    spinner = null;
                }

                if (!displayedFailureDialog && !foreground) {
                    displayedFailureDialog = true;
                    stopConnection();
                    finish();
                    return;
                }
                if (!displayedFailureDialog && foreground && !isFinishing()) {
                    displayedFailureDialog = true;
                    LimeLog.severe(stage + " failed: " + errorCode);

                    // If video initialization failed and the surface is still valid, display extra information for the user
                    if (stage.contains("video") && videoSurface != null && videoSurface.isValid()) {
                        messages.alert(getResources().getText(R.string.video_decoder_init_failed), Toast.LENGTH_LONG);
                    }

                    // A decoder that rejected the stream is this device's failure, not the host's, and
                    // no port or host advice applies to it.
                    DecoderSetupFailure decoderFailure = decoderRenderer == null ? null : decoderRenderer.getSetupFailure();
                    boolean decoderFailed = isDecoderStageFailure(decoderFailure, stage);
                    String dialogText = startupFailureText(Game.this, decoderFailure, stage, connectionErrorMessage, errorCode);

                    if (portFlags != 0 && !decoderFailed) {
                        dialogText += "\n\n" + getResources().getString(R.string.check_ports_msg) + "\n" +
                                MoonBridge.stringifyPortFlags(portFlags, "\n");
                    }

                    if (portTestResult != MoonBridge.ML_TEST_RESULT_INCONCLUSIVE && portTestResult != 0)  {
                        dialogText += "\n\n" + getResources().getString(R.string.nettest_text_blocked);
                    }

                    if (app.isSmallStreamInputOnly()) {
                        dialogText += "\n\n" + getString(R.string.input_only_small_stream_refused);
                    }

                    stopConnection();
                    showReconnectDialog(R.string.conn_error_title, dialogText);
                }
            }
        });
    }

    /** A recorded decoder setup failure at a video stage: this device's failure, not the host's. */
    public static boolean isDecoderStageFailure(DecoderSetupFailure decoderFailure, String stage) {
        return decoderFailure != null && stage.contains("video");
    }

    /** The stream dialog's text for a failed stage: the decoder's reason, else the connection error, else the host's. */
    public static String startupFailureText(Context context, DecoderSetupFailure decoderFailure, String stage,
                                            String connectionErrorMessage, int errorCode) {
        if (isDecoderStageFailure(decoderFailure, stage)) {
            return decoderFailure.message(context);
        }
        if (connectionErrorMessage != null) {
            return connectionErrorMessage;
        }
        return context.getString(errorCode == 0 ? R.string.startup_connection_failed :
                R.string.startup_host_error, errorCode);
    }

    static String terminationDescription(int errorCode) {
        String code = Math.abs(errorCode) > 1000 ? "0x" + Integer.toHexString(errorCode) : Integer.toString(errorCode);
        switch (errorCode) {
            case MoonBridge.ML_ERROR_NO_VIDEO_TRAFFIC: return code + " (no video traffic)";
            case MoonBridge.ML_ERROR_NO_VIDEO_FRAME: return code + " (no complete video frame)";
            case MoonBridge.ML_ERROR_UNEXPECTED_EARLY_TERMINATION: return code + " (ended right after starting)";
            case MoonBridge.ML_ERROR_PROTECTED_CONTENT: return code + " (protected content)";
            case MoonBridge.ML_ERROR_FRAME_CONVERSION: return code + " (frame conversion failed on the PC)";
            case MoonBridge.CLIENT_ERROR_VULKAN_RENDERER: return code + " (Vulkan renderer failed on this device)";
            case MoonBridge.CLIENT_ERROR_PACKED_444_REFUSED: return code + " (full chroma not available with this PC)";
            case MoonBridge.CLIENT_ERROR_DECODER: return code + " (video decoder failed on this device)";
            // This device's own code for a connection that ended without any reason from the PC.
            case -1: return code + " (connection lost, no reason from the PC)";
            default: return code + " (reported by the PC)";
        }
    }

    /** Large codes are hex values, small ones decimal. */
    private static String errorCodeText(int errorCode) {
        return Math.abs(errorCode) > 1000 ? Integer.toHexString(errorCode) : Integer.toString(errorCode);
    }

    @Override
    public void connectionTerminated(final int reportedErrorCode) {
        LimeLog.info("Stream disconnected");
        streamTerminated = true;
        final boolean exitRequested = hostExitRequested;
        if (reportedErrorCode == MoonBridge.ML_ERROR_GRACEFUL_TERMINATION) {
            LimeLog.info("Connection terminated: the PC ended the stream");
        } else if (exitRequested) {
            LimeLog.info("Connection terminated after this device ended the session: " +
                    terminationDescription(reportedErrorCode));
        } else {
            LimeLog.severe("Connection terminated: " + terminationDescription(reportedErrorCode));
        }
        // Some hosts close the transport before their graceful termination packet arrives.
        // Verify an idle host before either retrying or showing an error, even with recovery off.
        final boolean endedSession = !exitRequested && connected &&
                StreamRecoveryPolicy.transportTermination(reportedErrorCode) &&
                conn != null && conn.hasEndedSession();
        final int errorCode = exitRequested || endedSession ?
                MoonBridge.ML_ERROR_GRACEFUL_TERMINATION : reportedErrorCode;
        if (endedSession) LimeLog.info("Connection terminated: the PC ended the previous session");
        final int portFlags = MoonBridge.getPortFlagsFromTerminationErrorCode(errorCode);
        // A stream that never had video does not reconnect: its error (and the port test) shows at once.
        final boolean transientFailure = StreamRecoveryPolicy.recoversTermination(errorCode, videoStarted());
        // A renderer failure on this device says nothing about the network
        final int portTestResult = errorCode == MoonBridge.ML_ERROR_GRACEFUL_TERMINATION ||
                errorCode == MoonBridge.CLIENT_ERROR_VULKAN_RENDERER || errorCode == MoonBridge.CLIENT_ERROR_DECODER ||
                (transientFailure && prefConfig != null && prefConfig.autoReconnect) ?
                MoonBridge.ML_TEST_RESULT_INCONCLUSIVE :
                MoonBridge.testClientConnectivity(ServerHelper.CONNECTION_TEST_SERVER,443, portFlags);

        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (isFinishing() || isDestroyed()) {
                    return;
                }
                if (errorCode == MoonBridge.ML_ERROR_GRACEFUL_TERMINATION && stoppedStreamResumeIntent != null) {
                    // The PC ended a session that leaving the stream kept for resuming.
                    stoppedStreamResumeIntent = null;
                    finish();
                    return;
                }
                if (transientFailure && !StreamRecoveryPolicy.transportTermination(errorCode)) {
                    // Keep explicit media/early failures visible if a later recovery attempt
                    // finds the host idle. Transport startup retries carry this same flag.
                    getIntent().putExtra(EXTRA_RECOVERY_MEDIA_FAILURE, true);
                }
                recoveryBudgetExhausted = false;
                if (transientFailure && tryAutomaticRecovery()) return;
                final boolean recoveryExhausted = transientFailure && recoveryBudgetExhausted;
                if (!foreground && transientFailure) rememberStreamForResume(false);
                // Let the display go to sleep now
                getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);

                // Stop processing controller input
                if (controllerHandler != null) controllerHandler.stop();

                // Ungrab input
                touchInput.setInputGrabState(false);

                if (!displayedFailureDialog) {
                    displayedFailureDialog = true;
                    stopConnection();

                    // Display the error dialog if it was an unexpected termination.
                    // Otherwise, just finish the activity immediately.
                    if (errorCode != MoonBridge.ML_ERROR_GRACEFUL_TERMINATION) {
                        String message;

                        if (portTestResult != MoonBridge.ML_TEST_RESULT_INCONCLUSIVE && portTestResult != 0) {
                            // If we got a blocked result, that supersedes any other error message
                            message = getResources().getString(R.string.nettest_text_blocked);
                        }
                        else {
                            switch (errorCode) {
                                case MoonBridge.ML_ERROR_NO_VIDEO_TRAFFIC:
                                    message = getResources().getString(R.string.no_video_received_error);
                                    break;

                                case MoonBridge.ML_ERROR_NO_VIDEO_FRAME:
                                    MediaCodecDecoderRenderer frames = decoderRenderer;
                                    message = getResources().getString(frames != null && frames.isPyroWaveHlgLost() ?
                                            R.string.stream_hdr_pyrowave_hlg_lost : R.string.no_frame_received_error);
                                    break;

                                case MoonBridge.ML_ERROR_UNEXPECTED_EARLY_TERMINATION:
                                case MoonBridge.ML_ERROR_PROTECTED_CONTENT:
                                    message = getResources().getString(R.string.early_termination_error);
                                    break;

                                case MoonBridge.ML_ERROR_FRAME_CONVERSION:
                                    message = getResources().getString(R.string.frame_conversion_error);
                                    break;

                                case MoonBridge.CLIENT_ERROR_VULKAN_RENDERER:
                                    MediaCodecDecoderRenderer renderer = decoderRenderer;
                                    String failure = renderer != null ? renderer.getVulkanFailure() : null;
                                    message = getResources().getString(R.string.vulkan_renderer_failed,
                                            failure != null ? failure : getResources().getString(R.string.vulkan_failure_unknown));
                                    break;

                                case MoonBridge.CLIENT_ERROR_DECODER:
                                    MediaCodecDecoderRenderer decoder = decoderRenderer;
                                    String reason = decoder != null ? decoder.getDecoderFailure() : null;
                                    message = getResources().getString(StreamExitPolicy.decoderFailureMessage(
                                                    decoder != null && decoder.decoderFailedWithVulkan()),
                                            reason != null ? reason : getResources().getString(R.string.vulkan_failure_unknown));
                                    break;

                                default:
                                    message = getResources().getString(R.string.conn_terminated_msg) + "\n\n" +
                                            getResources().getString(R.string.error_code_prefix) + " " + errorCodeText(errorCode);
                                    if (prefConfig != null &&
                                            NetworkGrace.mayHaveGivenUp(prefConfig.networkGraceSeconds, errorCode)) {
                                        message = getResources().getString(R.string.network_grace_gave_up,
                                                prefConfig.networkGraceSeconds) + "\n\n" + message;
                                    }
                                    break;
                            }
                        }

                        if (portFlags != 0) {
                            message += "\n\n" + getResources().getString(R.string.check_ports_msg) + "\n" +
                                    MoonBridge.stringifyPortFlags(portFlags, "\n");
                        }

                        boolean portTestBlocked = portTestResult != MoonBridge.ML_TEST_RESULT_INCONCLUSIVE &&
                                portTestResult != 0;
                        boolean graceGaveUp = prefConfig != null &&
                                NetworkGrace.mayHaveGivenUp(prefConfig.networkGraceSeconds, errorCode);
                        if (foreground && (stoppedStreamResumeIntent != null ||
                                StreamExitPolicy.terminationNeedsAction(errorCode, portTestBlocked,
                                        graceGaveUp, recoveryExhausted))) {
                            showReconnectDialog(R.string.conn_terminated_title, message);
                        } else if (foreground) {
                            // The host went away or the connection was lost: nothing to act on, so
                            // go back to the list and say why where it outlives this screen. Android
                            // 12+ shows two lines of a text toast, so it is the title and the code.
                            Toast.makeText(getApplicationContext(),
                                    getString(R.string.conn_terminated_title) + "\n" +
                                            getString(R.string.error_code_prefix) + " " + errorCodeText(errorCode),
                                    Toast.LENGTH_LONG).show();
                            finish();
                        } else if (stoppedStreamResumeIntent == null) {
                            finish();
                        }
                    }
                    else {
                        finish();
                    }
                }
            }
        });
    }

    @Override
    public void connectionStatusUpdate(final int connectionStatus) {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (!connected || isFinishing() || isDestroyed()) {
                    return;
                }
                poorConnection = connectionStatus == MoonBridge.CONN_STATUS_POOR;
                // The lost-network notice stays until a network returns.
                if (prefConfig.disableWarnings || streamNetworkLost) {
                    return;
                }

                if (connectionStatus == MoonBridge.CONN_STATUS_POOR) {
                    if (prefConfig.bitrate > 5000) {
                        notificationOverlayView.setText(getResources().getString(R.string.slow_connection_msg));
                    }
                    else {
                        notificationOverlayView.setText(getResources().getString(R.string.poor_connection_msg));
                    }

                    requestedNotificationOverlayVisibility = View.VISIBLE;
                }
                else if (connectionStatus == MoonBridge.CONN_STATUS_OKAY) {
                    requestedNotificationOverlayVisibility = View.GONE;
                }

                if (!isHidingOverlays) {
                    notificationOverlayView.setVisibility(requestedNotificationOverlayVisibility);
                }
            }
        });
    }

    @Override
    public void streamNetworkLost() {
        runOnUiThread(() -> {
            if (!connected || displayedFailureDialog || isFinishing() || isDestroyed()) return;
            // A frozen picture is never silent: say what happened, even with warnings off.
            // It promises a reconnect only when one will follow (the connection's timeout, or a
            // return it cannot carry on over, recovers it).
            streamNetworkLost = true;
            notificationOverlayView.setText(getString(recoversAutomatically() &&
                    prefConfig.networkGraceSeconds * 1000 <= NetworkReturnRecovery.PEER_TIMEOUT_MS ?
                    R.string.stream_network_lost_reconnecting : R.string.stream_network_lost));
            requestedNotificationOverlayVisibility = View.VISIBLE;
            if (!isHidingOverlays) notificationOverlayView.setVisibility(View.VISIBLE);
        });
    }

    @Override
    public void streamNetworkReturned(final int change, final boolean metered) {
        runOnUiThread(() -> {
            // A return the connection carried on over (no route yet) is reported again once
            // the engine's answer changes (the address arrived): only the first clears the notice.
            if (streamNetworkLost) {
                streamNetworkLost = false;
                requestedNotificationOverlayVisibility = View.GONE;
                if (!isHidingOverlays) notificationOverlayView.setVisibility(View.GONE);
            }
            // Roamed, or carrying on (the same address back, or no route yet): the stream goes on,
            // and its own timeout decides if it cannot.
            // Like a drop, only a stream whose video started reconnects (desktop: reconnects_on_its_own).
            if (!connected || displayedFailureDialog || isFinishing() || isDestroyed() || prefConfig == null ||
                    !videoStarted() || !NetworkReturnRecovery.reconnectsAtOnce(change == MoonBridge.NETWORK_CANNOT_CONTINUE, metered,
                            prefConfig.roamingMetered, prefConfig.autoReconnect,
                            prefConfig.networkGraceSeconds * 1000)) return;
            // The connection cannot carry on over this network and would only time out first.
            LimeLog.info("The network is back on another address the stream did not roam to: reconnecting now");
            if (!tryAutomaticRecovery(false, true)) {
                LimeLog.warning("The network is back, but no automatic reconnection is possible now");
            }
        });
    }

    @Override
    public void connectionStarted() {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (!connecting || isFinishing() || isDestroyed()) {
                    return;
                }
                if (spinner != null) {
                    spinner.dismiss();
                    spinner = null;
                }

                connected = true;
                conn.refreshNativeHostControl();
                getIntent().removeExtra(EXTRA_RECOVERY_MEDIA_FAILURE);
                bitrateHandler.removeCallbacks(sampleStreamHealth);
                bitrateHandler.postDelayed(sampleStreamHealth, 1000);
                bitrateHandler.removeCallbacks(pollContentRateVote);
                if (prefConfig.contentRateFrameRate) bitrateHandler.postDelayed(pollContentRateVote, 1000);
                startRefreshSwitchPolling();
                if (wifiDiagnostics == null) wifiDiagnostics = new com.limelight.utils.WifiDiagnostics(Game.this);
                wifiDiagnostics.start();
                onStreamConnected(SystemClock.elapsedRealtime());
                recoveryAppId = app.getControl() == NvApp.Control.NONE ? app.getAppId() : conn.getHostDetails().runningGameId;
                recoveryAppUuid = app.getControl() == NvApp.Control.NONE ? app.getAppUuid() : conn.getHostDetails().runningGameUuid;
                touchInput.updateGameTouchProfile();
                controllerHandler.resendControllerArrivals();
                overlays.updateHardwareMetricsPolling();
                LimeLog.info("Stream connected");
                connecting = false;
                if (!isRecoveryAttempt()) {
                    com.limelight.update.UpdateFlow.notifyForStream(Game.this, conn.getHostDetails(),
                            () -> foreground && connected, text -> messages.alert(text, Toast.LENGTH_LONG));
                }
                if (app.getRole() != NvApp.Role.INPUT_ONLY && !app.isControlAction()) {
                    displayCapsReporter = new DisplayCapsReporter(MoonBridge::sendDisplayCaps);
                    reportDisplayCaps();
                }
                startReconfigureTracking();
                requestHostTiming();
                requestLinkNotice();
                keyboardInput.requestTextInput();
                startClipboardSync();
                touchInput.updateHostCursorShapes();
                if (!conn.canSendInput()) {
                    if (gameTouchControls != null) gameTouchControls.hideForPip(true);
                    if (virtualController != null) {
                        virtualController.hide();
                    }
                    if (modernController != null) modernController.hide();
                    if (micButton != null) micButton.setSuppressed(true);
                    controllerHandler.stop();
                    touchInput.setInputGrabState(false);
                }
                if (app.isSmallStreamInputOnly() && externalPresentation == null) {
                    ((TextView) findViewById(R.id.inputOnlyStatus)).setText(getString(
                            R.string.input_only_small_stream_status, InputOnlyPolicy.estimatedKbps() / 1000f));
                }
                findViewById(R.id.inputOnlyStatus).setVisibility(
                        app.getRole() == NvApp.Role.INPUT_ONLY || externalPresentation != null ? View.VISIBLE : View.GONE);
                // A link test before the launch may have started the stream lower than the setting.
                currentBitrate = app.isSmallStreamInputOnly() ? prefConfig.bitrate : conn.getStartBitrateKbps();
                overlays.notePyroWaveFallback();
                startStreamBitrate();
                startAdaptiveQuality();
                updatePipAutoEnter();

                if (!conn.getHostDetails().hasPermission(ComputerDetails.PERMISSION_INPUT)) {
                    messages.alert(R.string.stream_input_denied, Toast.LENGTH_LONG);
                }

                // Hide the mouse cursor now after a short delay.
                // Doing it before dismissing the spinner seems to be undone
                // when the spinner gets displayed. On Android Q, even now
                // is too early to capture. We will delay a second to allow
                // the spinner to dismiss before capturing.
                bitrateHandler.postDelayed(new Runnable() {
                    @Override
                    public void run() {
                        if (foreground && connected && streamMenu == null && !isFinishing()) {
                            touchInput.setInputGrabState(true);
                        }
                    }
                }, 500);

                // Keep the display on
                getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);

                // Update GameManager state to indicate we're in game
                UiHelper.notifyStreamConnected(Game.this);

                hideSystemUi(1000);

                // The first stream says once how to open the stream menu, after the start settles.
                if (!com.limelight.ui.StreamMenuHint.seen(Game.this)) {
                    bitrateHandler.removeCallbacks(showStreamMenuHint);
                    bitrateHandler.postDelayed(showStreamMenuHint, 1500);
                }
            }
        });

        // Report this shortcut being used (off the main thread to prevent ANRs)
        ComputerDetails computer = new ComputerDetails();
        computer.name = pcName;
        computer.uuid = Game.this.getIntent().getStringExtra(EXTRA_PC_UUID);
        ShortcutHelper shortcutHelper = new ShortcutHelper(this);
        shortcutHelper.reportComputerShortcutUsed(computer);
        if (appName != null) {
            // This may be null if launched from the "Resume Session" PC context menu item
            shortcutHelper.reportGameLaunched(computer, app);
        }
    }

    @Override
    public void displayMessage(final String message) {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                connectionErrorMessage = message;
                if (connected) {
                    messages.alert(message, Toast.LENGTH_LONG);
                }
            }
        });
    }

    @Override
    public void launchConfirmationRequired(LaunchConfirmation confirmation) {
        runOnUiThread(() -> {
            if (!foreground || isFinishing() || confirmation.getState(LaunchConfirmation.nowMs()) != LaunchConfirmation.State.WAITING) {
                confirmation.cancel();
                return;
            }
            if (spinner != null) {
                spinner.dismiss();
                spinner = null;
            }
            pendingConfirmation = confirmation;
            launchConfirmationDialog = new com.google.android.material.dialog.MaterialAlertDialogBuilder(
                    new androidx.appcompat.view.ContextThemeWrapper(this, R.style.StreamTheme))
                    .setTitle(R.string.launch_confirm_title)
                    .setMessage(launchConfirmationMessage(confirmation))
                    .setPositiveButton(R.string.launch_confirm, (dialog, which) -> {
                        confirmation.confirm(LaunchConfirmation.nowMs());
                        spinner = SpinnerDialog.displayDialog(this, getString(R.string.conn_establishing_title),
                                getString(R.string.conn_establishing_msg), true);
                    })
                    .setNegativeButton(android.R.string.cancel, (dialog, which) -> {
                        confirmation.cancel();
                        finish();
                    }).create();
            launchConfirmationDialog.setOnCancelListener(dialog -> {
                confirmation.cancel();
                finish();
            });
            launchConfirmationDialog.setCanceledOnTouchOutside(false);
            launchConfirmationDialog.show();
        });
    }

    /** Names the running game and this one when the start knew both; otherwise "the running app". */
    private String launchConfirmationMessage(LaunchConfirmation confirmation) {
        if (confirmation.isTerminate()) {
            return getString(R.string.launch_confirm_terminate);
        }
        String replaced = getIntent().getStringExtra(EXTRA_REPLACED_APP_NAME);
        return replaced != null && appName != null
                ? getString(R.string.launch_confirm_replace_named, replaced, appName)
                : getString(R.string.launch_confirm_replace);
    }

    @Override
    public void launchConfirmationFinished() {
        runOnUiThread(() -> {
            pendingConfirmation = null;
            if (launchConfirmationDialog != null) {
                launchConfirmationDialog.dismiss();
                launchConfirmationDialog = null;
            }
        });
    }

    @Override
    public void launchActionCompleted(String message) {
        runOnUiThread(() -> {
            if (!isFinishing()) {
                messages.alert(message, Toast.LENGTH_LONG);
                finish();
            }
        });
    }

    // The connection's own once-per-PC facts: a virtual display, 4K or HDR the PC cannot offer.
    @Override
    public void displayTransientMessage(final String message) {
        runOnUiThread(() -> {
            if (prefConfig.disableWarnings) messages.note(message);
            else messages.countedNotice(message);
        });
    }

    @Override
    public void rumble(short controllerNumber, short lowFreqMotor, short highFreqMotor) {
        LimeLog.info(String.format((Locale)null, "Rumble on gamepad %d: %04x %04x", controllerNumber, lowFreqMotor, highFreqMotor));

        controllerHandler.handleRumble(controllerNumber, lowFreqMotor, highFreqMotor);
    }

    @Override
    public void rumbleTriggers(short controllerNumber, short leftTrigger, short rightTrigger) {
        LimeLog.info(String.format((Locale)null, "Rumble on gamepad triggers %d: %04x %04x", controllerNumber, leftTrigger, rightTrigger));

        controllerHandler.handleRumbleTriggers(controllerNumber, leftTrigger, rightTrigger);
    }

    @Override
    public void setHdrMode(boolean enabled, byte[] hdrMetadata) {
        LimeLog.info("Display HDR mode: " + (enabled ? "enabled" : "disabled"));
        decoderRenderer.setHdrMode(enabled, hdrMetadata);
    }

    @Override
    public void setMotionEventState(short controllerNumber, byte motionType, short reportRateHz) {
        controllerHandler.handleSetMotionEventState(controllerNumber, motionType, reportRateHz);
    }

    @Override
    public void setControllerLED(short controllerNumber, byte r, byte g, byte b) {
        controllerHandler.handleSetControllerLED(controllerNumber, r, g, b);
    }

    @Override
    public void linkNotice(int state, int ceilingKbps, int currentKbps) {
        LinkNoticeStatus status = LinkNoticeStatus.parse(state, ceilingKbps, currentKbps);
        runOnUiThread(() -> {
            linkNotice = status;
            if (performanceOverlayView != null) overlays.updatePerformanceOverlay();
        });
    }

    @Override
    public void rateChange(RateChangeResult result) {
        runOnUiThread(() -> onRateChange(result));
    }

    @Override
    public void textInput(int kind, int inputType, int flags, int element) {
        runOnUiThread(() -> keyboardInput.onHostTextField(kind, inputType, flags, element));
    }

    @Override
    public void clipboardSync(String text) {
        runOnUiThread(() -> {
            if (clipboardSync != null) clipboardSync.received(text);
        });
    }

    @Override
    public void clipboardSyncRefused(int reason) {
        runOnUiThread(() -> {
            if (clipboardSync != null) clipboardSync.refused(reason);
        });
    }

    @Override
    public void cursorShape(byte[] message) {
        HostCursorShapes shapes = hostCursorShapes;
        if (shapes != null) shapes.accept(message);
    }

    @Override
    public void controllerAudio(int kind, byte[] payload) { controllerHandler.handleControllerAudio(kind, payload); }

    @Override
    public void controllerHaptics(short controllerNumber, int sequence, byte[] pcm) {
        controllerHandler.handleControllerHaptics(controllerNumber, sequence, pcm);
    }

    @Override
    public void setAdaptiveTriggers(short controllerNumber, byte eventFlags, byte typeLeft,
                                    byte typeRight, byte[] left, byte[] right) {
        controllerHandler.handleSetAdaptiveTriggers(controllerNumber, eventFlags, typeLeft, typeRight, left, right);
    }

    @Override
    public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) {
        startConnectionOnSurface();
    }

    private void startConnectionOnSurface() {
        if (!surfaceCreated || isFinishing() || isDestroyed()) {
            return;
        }

        if (!attemptedConnection) {
            if (isRecoveryAttempt()) {
                long deadlineFrom = isTripResume() ?
                        getIntent().getLongExtra(EXTRA_TRIP_RESUME_STARTED, 0) : recoveryPolicy.startedAt();
                long remaining = StreamRecoveryPolicy.WINDOW_MS -
                        (SystemClock.elapsedRealtime() - deadlineFrom);
                if (remaining <= 0) {
                    if (spinner != null) { spinner.dismiss(); spinner = null; }
                    showReconnectDialog(R.string.conn_terminated_title, getString(R.string.stream_recovery_exhausted));
                    return;
                }
                bitrateHandler.postDelayed(recoveryTimeout, remaining);
            }
            attemptedConnection = true;
            connecting = true;
            LimeLog.info("Stream starting");

            // Update GameManager state to indicate we're "loading" while connecting
            UiHelper.notifyStreamConnecting(Game.this);

            decoderRenderer.setRenderTarget(videoSurface, renderView.getWidth(), renderView.getHeight());
            if (app.isSmallStreamInputOnly()) {
                conn.start(new SilentAudioRenderer(), new DiscardingDecoderRenderer(), Game.this);
            } else {
                conn.start(new AndroidAudioRenderer(Game.this, prefConfig.enableAudioFx,
                                prefConfig.usbDualSenseHaptics ? controllerHandler.headphoneAudioSink() : null),
                        decoderRenderer, Game.this);
            }
        }
    }

    @Override
    public void surfaceCreated(SurfaceHolder holder) {
        configureVideoSurface(holder.getSurface());
    }

    private void configureVideoSurface(Surface surface) {
        videoSurface = surface;
        appSurfaceFrameRateVote = null;
        surfaceCreated = true;
        if (resumeStoppedStreamIfNeeded()) return;
        LimeLog.info("Video surface: " + (prefConfig.useTextureView ? "TextureView" : "SurfaceView") +
                ", pacing: " + prefConfig.framePacing + ", newest frame: " + prefConfig.dropLateFrames);

        if (prefConfig.useTextureView) {
            logDisplayRefreshPolicy(getVideoDisplay());
            return;
        }

        voteSurfaceFrameRate(surface);

        // Disable producer throttling on the underlying surface for reduced latency
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.CINNAMON_BUN) {
            surface.setProducerThrottlingEnabled(false);
        }
    }

    private void refreshSurfaceFrameRateVote() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R && surfaceCreated && videoSurface != null &&
                videoSurface.isValid() && !prefConfig.useTextureView) {
            voteSurfaceFrameRate(videoSurface);
        }
    }

    private void voteSurfaceFrameRate(Surface surface) {
        if (useArr && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            // The stream rate, seamless only: ARR's steps are seamless, and a 0 vote would leave
            // the game at Android's 60 Hz default for games (see arrVote). arrHalfRate watches for it.
            int exactRate = decoderRenderer == null ? 0 : decoderRenderer.getStreamFrameRateMilliHz();
            float vote = com.limelight.binding.video.DisplayFrameRatePolicy.arrVote(
                    exactRate > 0 ? exactRate / 1000f : prefConfig.fps, panelMaxRefreshRate);
            applySurfaceFrameRate(surface, vote, Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS);
            arrHalfRate.reset();
            LimeLog.info("VRR: ARR vote " + vote + " Hz (stream rate, seamless only)");
            logDisplayRefreshPolicy(getVideoDisplay());
            return;
        }

        applySurfaceFrameRate(surface, baseSurfaceFrameRate(), Surface.CHANGE_FRAME_RATE_ALWAYS);
        // The base vote replaced any content rate vote; make it again.
        contentRateVote.invalidate();
        logDisplayRefreshPolicy(getVideoDisplay());
    }

    // The vote for the stream before the content rate is known, or when it is not followed.
    private float baseSurfaceFrameRate() {
        float desiredFrameRate;

        // Android will pick the lowest matching refresh rate for a given frame rate value, so we want
        // to report the true FPS value if refresh rate reduction is enabled. We also report the true
        // FPS value if there's no suitable matching refresh rate. In that case, Android could try to
        // select a lower refresh rate that avoids uneven pull-down (ex: 30 Hz for a 60 FPS stream on
        // a display that maxes out at 50 Hz).
        if (prefConfig.vrr) {
            desiredFrameRate = panelMaxRefreshRate;
        }
        else if (mayReduceRefreshRate() || desiredRefreshRate < prefConfig.fps) {
            int exactRate = decoderRenderer == null ? 0 : decoderRenderer.getStreamFrameRateMilliHz();
            desiredFrameRate = exactRate > 0 ? exactRate / 1000f : prefConfig.fps;
        }
        else {
            // Otherwise, we will pretend that our frame rate matches the refresh rate we picked in
            // prepareDisplayForRendering(). This will usually be the highest refresh rate that our
            // frame rate evenly divides into, which ensures the lowest possible display latency.
            desiredFrameRate = desiredRefreshRate;
        }
        return desiredFrameRate;
    }

    private void applySurfaceFrameRate(Surface surface, float frameRate, int changeStrategy) {
        // Tell the OS about our frame rate to allow it to adapt the display refresh rate appropriately
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            // The stream's own vote changes the rate even if it's not seamless, since
            // prepareDisplayForRendering() will not set the display mode on S+ if it only differs
            // by the refresh rate. It depends on us to trigger the frame rate switch here.
            surface.setFrameRate(frameRate,
                    Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE,
                    changeStrategy);
            appSurfaceFrameRateVote = frameRate;
            appSurfaceSeamlessOnly = changeStrategy == Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS;
        }
        else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            // No strategy on API 30: the two-argument vote only ever switches seamlessly.
            surface.setFrameRate(frameRate,
                    Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE);
            appSurfaceFrameRateVote = frameRate;
            appSurfaceSeamlessOnly = true;
        }
    }

    private void startRefreshSwitchPolling() {
        if (refreshSwitchMonitor == null) {
            refreshSwitchMonitor = new com.limelight.binding.video.RefreshSwitchMonitor(this);
        }
        bitrateHandler.removeCallbacks(pollRefreshSwitch);
        bitrateHandler.postDelayed(pollRefreshSwitch, 1000);
    }

    private void stopRefreshSwitchPolling() {
        bitrateHandler.removeCallbacks(pollRefreshSwitch);
        arrHalfRate.reset();
        messages.status("refresh", null);
        messages.status("arr", null);
        messages.status("hdr", null);
    }

    // When the screen has stayed off the rate voted for, says why on the overlay's status line.
    private void checkRefreshSwitch() {
        if (refreshSwitchMonitor == null) return;
        String text = refreshSwitchMonitor.poll(SystemClock.uptimeMillis(), getVideoDisplay(),
                surfaceCreated && !useArr ? appSurfaceFrameRateVote : null, appSurfaceSeamlessOnly);
        if (messages.status("refresh", text)) overlays.updatePerformanceOverlay();
    }

    // Under ARR the refresh switch check above cannot tell what the screen runs at, so watch what it
    // shows: about half the rendered frames for 3 s, with the display at about half the stream rate,
    // means Android holds the stream at half its rate.
    private void checkArrRate() {
        if (!useArr || decoderRenderer == null) return;
        float[] rates = decoderRenderer.getFrameRates();
        float rendered = PerformanceOverlay.renderedFps(rates);
        float presented = PerformanceOverlay.presentedFps(rates);
        Display display = getVideoDisplay();
        float displayHz = display == null ? 0 : display.getRefreshRate();
        int exactRate = decoderRenderer.getStreamFrameRateMilliHz();
        float streamHz = exactRate > 0 ? exactRate / 1000f : prefConfig.fps;
        boolean was = arrHalfRate.flagged();
        boolean flagged = arrHalfRate.update(SystemClock.uptimeMillis(), rendered, presented, displayHz, streamHz);
        if (flagged && !was) {
            LimeLog.warning("VRR: ARR shows " + presented + " of " + rendered + " fps rendered for " +
                    com.limelight.binding.video.DisplayFrameRatePolicy.ArrHalfRateCheck.HOLD_MS / 1000 +
                    " s, display at " + displayHz + " Hz: the screen is held at half the stream rate (vote " +
                    appSurfaceFrameRateVote + " Hz)");
        } else if (!flagged && was) {
            LimeLog.info("VRR: ARR shows the rendered frames again (" + presented + " of " + rendered + " fps)");
        }
        String text = flagged ? getString(R.string.stream_vrr_arr_half_rate, Math.round(presented), Math.round(rendered)) : null;
        if (messages.status("arr", text)) overlays.updatePerformanceOverlay();
    }

    // Render through Vulkan can't keep up on this display: say so once per PC and on the status line.
    // The renderer stays on and the setting stays; turning it off is the user's choice.
    private void checkVulkanPace() {
        if (decoderRenderer != null && decoderRenderer.takeVulkanBehind()) {
            overlays.hostNotice(HostNotices.Notice.VULKAN_BEHIND, R.string.stream_vulkan_behind);
        }
    }

    // HDR was granted and then not delivered: say so on the status line for as long as it lasts,
    // and log it. A refusal before the stream is already noted by refuseHdr, not repeated here.
    private void checkHdrStatus() {
        String text = null;
        if (streamHdr && decoderRenderer != null) {
            int format = decoderRenderer.getActiveVideoFormat();
            HdrPolicy.Status status = format == 0 ? HdrPolicy.Status.WAITING : HdrPolicy.drawnStatus(HdrPolicy.status(true,
                    (format & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0, decoderRenderer.getHostHdrReport(),
                    decoderRenderer.getHdrMetadataState(), decoderRenderer.getDecodedTransfer(),
                    decoderRenderer.getStreamTransfer(), HdrPolicy.hlgOnly(HdrPolicy.displayHdrTypes(getVideoDisplay()))),
                    decoderRenderer.isVulkanHdrToneMapped());
            int res = hdrStatusText(status, decoderRenderer.isPyroWaveHlgNeedingHostUpdate(),
                    decoderRenderer.isPyroWaveHlgRefusedByDisplay());
            if (status == HdrPolicy.Status.SHOWN_AS_SDR) {
                text = getString(res, HdrFormatPolicy.baseLayerName(decoderRenderer.getAnnouncedHdrFormat()));
            } else if (res != 0) {
                text = getString(res);
            }
            // The window took HLG before the stream but does not show it as HLG (the renderer and
            // MediaCodecDecoderRenderer log why).
            if (decoderRenderer.isPyroWaveHlgLost()) text = getString(R.string.stream_hdr_pyrowave_hlg_lost);
            // Plain words, once per PC, for what the Vulkan renderer cannot show of this screen's HDR.
            int limit = vulkanHdrLimitText(decoderRenderer.takeVulkanHdrLimit());
            if (limit != 0) overlays.hostNotice(HostNotices.Notice.VULKAN_HDR_LIMIT, limit);
            // Once per stream on the status line (and once per PC as a message): the output window
            // refused HDR10+, so the picture keeps HDR10's static metadata alone.
            if (decoderRenderer.takeVulkanHdr10PlusRefused()) {
                LimeLog.warning("HDR10+ refused by the Vulkan renderer's window");
                overlays.hostNotice(HostNotices.Notice.VULKAN_HDR10_PLUS_REFUSED, R.string.stream_hdr_vulkan_no_hdr10_plus);
            }
            // And for what PyroWave cannot carry; Dolby Vision names what the stream is instead.
            int pyroWaveLimit = pyroWaveHdrLimitText(decoderRenderer.takePyroWaveHdrLimit());
            if (pyroWaveLimit != 0) {
                messages.notice(getIntent().getStringExtra(EXTRA_PC_UUID), HostNotices.Notice.PYROWAVE_HDR_LIMIT,
                        getString(pyroWaveLimit, HdrFormatPolicy.baseLayerName(decoderRenderer.getAnnouncedHdrFormat())));
            }
            if (status != lastHdrStatus) {
                lastHdrStatus = status;
                if (res != 0) LimeLog.warning("HDR requested but " + status);
            }
        }
        if (streamSdr10 && decoderRenderer != null) {
            int format = decoderRenderer.getActiveVideoFormat();
            boolean tenBit = (format & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0;
            Boolean report = decoderRenderer.getHostHdrReport();
            if (Sdr10Policy.status(tenBit, report) == Sdr10Policy.Status.HOST_HDR) {
                text = getString(R.string.stream_sdr10_host_hdr);
            }
            Sdr10Policy.HostHdrStep step = Sdr10Policy.hostHdrStep(tenBit, report, sdr10LastHostHdr);
            if (step == Sdr10Policy.HostHdrStep.SAY) {
                LimeLog.warning("10-bit SDR requested but the PC sends HDR");
                overlays.hostNotice(HostNotices.Notice.SDR10_HOST_HDR, R.string.stream_sdr10_host_hdr);
            } else if (step == Sdr10Policy.HostHdrStep.REARM) {
                // The PC's display is in SDR now; if it goes to HDR again, say so again.
                HostNotices.reset(this, getIntent().getStringExtra(EXTRA_PC_UUID), HostNotices.Notice.SDR10_HOST_HDR);
            }
            if (tenBit && report != null) sdr10LastHostHdr = report;
        }
        if (messages.status("hdr", text)) overlays.updatePerformanceOverlay();
    }

    static int hdrStatusText(HdrPolicy.Status status) {
        return hdrStatusText(status, false, false);
    }

    /**
     * @param pyroWaveHostUpdate an HLG-only screen gets HDR10 on PyroWave because the host offers
     *                           no HLG there (MediaCodecDecoderRenderer.isPyroWaveHlgNeedingHostUpdate)
     * @param pyroWaveHlgRefused the same because this display's window refused HLG
     *                           (MediaCodecDecoderRenderer.isPyroWaveHlgRefusedByDisplay)
     */
    static int hdrStatusText(HdrPolicy.Status status, boolean pyroWaveHostUpdate, boolean pyroWaveHlgRefused) {
        if (status == HdrPolicy.Status.PQ_ON_HLG_SCREEN && pyroWaveHostUpdate) {
            return R.string.stream_hdr_pq_on_hlg_screen_host_update;
        }
        if (status == HdrPolicy.Status.PQ_ON_HLG_SCREEN && pyroWaveHlgRefused) {
            return R.string.stream_hdr_pq_on_hlg_screen_display_refused;
        }
        switch (status) {
            case NOT_NEGOTIATED: return R.string.stream_hdr_not_negotiated;
            case HOST_SDR: return R.string.stream_hdr_host_sdr;
            case NO_METADATA: return R.string.stream_hdr_no_metadata;
            case OUTPUT_NOT_PQ: return R.string.stream_hdr_output_not_pq;
            case PQ_ON_HLG_SCREEN: return R.string.stream_hdr_pq_on_hlg_screen;
            case SHOWN_AS_SDR: return R.string.stream_hdr_vulkan_shown_as_sdr;
            default: return 0;
        }
    }

    static int vulkanHdrLimitText(HdrFormatPolicy.DrawnLimit limit) {
        switch (limit) {
            case DOLBY_VISION: return R.string.stream_hdr_vulkan_no_dolby_vision;
            default: return 0;
        }
    }

    /** The once-per-PC words for a PyroWave limit; the Dolby Vision text takes HDR10 or HLG. */
    static int pyroWaveHdrLimitText(HdrFormatPolicy.DrawnLimit limit) {
        switch (limit) {
            case DOLBY_VISION: return R.string.stream_hdr_pyrowave_no_dolby_vision;
            case HDR10_PLUS: return R.string.stream_hdr_pyrowave_no_hdr10_plus;
            default: return 0;
        }
    }

    private boolean followsContentRate() {
        return com.limelight.binding.video.DisplayFrameRatePolicy.followsContentRate(Build.VERSION.SDK_INT,
                prefConfig.contentRateFrameRate,
                MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_CLOCK_SYNC),
                useArr, prefConfig.vrr, prefConfig.useTextureView);
    }

    // Moves the surface's frame rate vote to the rate the game presents at once that held steady,
    // seamless modes only, so a vote never blanks the screen during the stream.
    private void voteContentRate() {
        if (!surfaceCreated || videoSurface == null || !videoSurface.isValid() || decoderRenderer == null ||
                !followsContentRate()) {
            return;
        }
        long now = SystemClock.uptimeMillis();
        int contentMilliHz = decoderRenderer.getContentRateMilliHz();
        if (contentRateVote.withdraw(now, contentMilliHz)) {
            // The game no longer holds a rate: back to the vote the stream would make.
            if (keepsVrrVote(baseSurfaceFrameRate())) return;
            applySurfaceFrameRate(videoSurface, baseSurfaceFrameRate(), Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS);
            LimeLog.info("Content frame rate vote withdrawn");
            logDisplayRefreshPolicy(getVideoDisplay());
            return;
        }
        float contentHz = contentRateVote.next(now, contentMilliHz);
        if (contentHz <= 0) return;
        // With VRR (no ARR here) the base vote is the panel maximum. With a lower refresh rate allowed
        // the vote is as without VRR. Otherwise it goes lower only when the game's rate would judder at
        // the maximum, to a seamless multiple of it: even frame timing for a little more wait for the
        // next refresh (about 1.4 ms on average for 90 Hz instead of 120, not measured).
        float vote = prefConfig.vrr ?
                com.limelight.binding.video.DisplayFrameRatePolicy.vrrContentVote(contentHz, panelMaxRefreshRate,
                        mayReduceRefreshRate(),
                        com.limelight.binding.video.DisplayFrameRatePolicy.seamlessRates(getVideoDisplay())) :
                com.limelight.binding.video.DisplayFrameRatePolicy.contentVote(contentHz,
                        baseSurfaceFrameRate(), mayReduceRefreshRate(), panelMaxRefreshRate);
        if (keepsVrrVote(vote)) return;
        applySurfaceFrameRate(videoSurface, vote, Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS);
        LimeLog.info("Content frame rate vote: " + vote + " Hz (game at " + contentHz + " Hz)");
        logDisplayRefreshPolicy(getVideoDisplay());
    }

    // With VRR a content vote that equals the vote in force (the panel maximum the stream started
    // with) is not made again: re-voting it seamless-only would change the reason the status line
    // gives when the screen never reached that rate (a TV that needs Match content frame rate).
    private boolean keepsVrrVote(float vote) {
        return prefConfig.vrr && appSurfaceFrameRateVote != null && Math.abs(vote - appSurfaceFrameRateVote) < 0.01f;
    }

    @Override
    public void surfaceDestroyed(SurfaceHolder holder) {
        destroyVideoSurface();
    }

    private void destroyVideoSurface() {
        if (!surfaceCreated) {
            return;
        }
        surfaceCreated = false;
        videoSurface = null;
        appSurfaceFrameRateVote = null;
        if (attemptedConnection && decoderRenderer != null) {
            // Stop rendering synchronously before TextureView releases its surface.
            // Reconnecting an external output still tears down the connection on a worker.
            decoderRenderer.prepareForStop();
        }
        if (externalPresentation != null && !isFinishing() && !isDestroyed()) {
            returnVideoToDevice();
            return;
        }
        if (attemptedConnection) {
            rememberStreamForResume(true);
            displayedFailureDialog = true;
            stopConnection();
            // The stopped decoder cannot be reused. Resume the captured game in a fresh
            // activity when foreground returns; leaving spends nothing from the recovery budget.
            if (finishesWhenStopped()) finish();
        }
    }

    @Override
    public void onSurfaceTextureAvailable(SurfaceTexture texture, int width, int height) {
        texture.setDefaultBufferSize(prefConfig.upscalingMode == UpscalingPolicy.Mode.OFF ? prefConfig.width : width,
                prefConfig.upscalingMode == UpscalingPolicy.Mode.OFF ? prefConfig.height : height);
        configureVideoSurface(new Surface(texture));
        startConnectionOnSurface();
    }

    @Override
    public void onSurfaceTextureSizeChanged(SurfaceTexture texture, int width, int height) {
        texture.setDefaultBufferSize(prefConfig.upscalingMode == UpscalingPolicy.Mode.OFF ? prefConfig.width : width,
                prefConfig.upscalingMode == UpscalingPolicy.Mode.OFF ? prefConfig.height : height);
    }

    @Override
    public boolean onSurfaceTextureDestroyed(SurfaceTexture texture) {
        Surface surface = videoSurface;
        destroyVideoSurface();
        if (surface != null) surface.release();
        return true;
    }

    @Override
    public void onSurfaceTextureUpdated(SurfaceTexture texture) {
    }

    @Override
    public void mouseMove(int deltaX, int deltaY) {
        touchInput.mouseMove(deltaX, deltaY);
    }

    @Override
    public void mouseButtonEvent(int buttonId, boolean down) {
        touchInput.mouseButtonEvent(buttonId, down);
    }

    @Override
    public void mouseVScroll(byte amount) {
        conn.sendMouseScroll(amount);
    }

    @Override
    public void mouseHScroll(byte amount) {
        conn.sendMouseHScroll(amount);
    }

    @Override
    public void keyboardEvent(boolean buttonDown, short keyCode) {
        keyboardInput.keyboardEvent(buttonDown, keyCode);
    }

    @Override
    public void onSystemUiVisibilityChange(int visibility) {
        // Don't do anything if we're not connected
        if (!connected) {
            return;
        }

        // This flag is set for all devices
        if ((visibility & View.SYSTEM_UI_FLAG_FULLSCREEN) == 0) {
            hideSystemUi(2000);
        }
        else if ((visibility & View.SYSTEM_UI_FLAG_HIDE_NAVIGATION) == 0) {
            hideSystemUi(2000);
        }
    }

    @Override
    public void onPerfUpdate(final String video, final String network, final String decode, final CharSequence statisticsText,
                             final CharSequence compactText) {
        overlays.onPerfUpdate(video, network, decode, statisticsText, compactText);
    }

    @Override
    public void onUsbPermissionPromptStarting() {
        // Disable PiP auto-enter while the USB permission prompt is on-screen. This prevents
        // us from entering PiP while the user is interacting with the OS permission dialog.
        suppressPipRefCount++;
        updatePipAutoEnter();
    }

    @Override
    public void onUsbPermissionPromptCompleted() {
        suppressPipRefCount--;
        updatePipAutoEnter();
    }

    @Override
    public boolean onKey(View view, int keyCode, KeyEvent keyEvent) {
        switch (keyEvent.getAction()) {
            case KeyEvent.ACTION_DOWN:
                return handleKeyDown(keyEvent);
            case KeyEvent.ACTION_UP:
                return handleKeyUp(keyEvent);
            case KeyEvent.ACTION_MULTIPLE:
                return keyboardInput.handleKeyMultiple(keyEvent);
            default:
                return false;
        }
    }
}
