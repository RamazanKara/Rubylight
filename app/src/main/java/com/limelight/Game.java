package com.limelight;


import com.limelight.binding.PlatformBinding;
import com.limelight.binding.audio.AndroidAudioRenderer;
import com.limelight.binding.audio.SilentAudioRenderer;
import com.limelight.binding.audio.MicrophoneStatus;
import com.limelight.binding.input.ControllerButtonMap;
import com.limelight.binding.input.ControllerHandler;
import com.limelight.binding.input.KeyboardLayoutTag;
import com.limelight.binding.input.KeyboardTranslator;
import com.limelight.binding.input.MouseDeltaAccumulator;
import com.limelight.binding.input.capture.InputCaptureManager;
import com.limelight.binding.input.capture.InputCaptureProvider;
import com.limelight.binding.input.touch.AbsoluteTouchContext;
import com.limelight.binding.input.touch.PointerAcceleration;
import com.limelight.binding.input.touch.RelativeTouchContext;
import com.limelight.binding.input.touch.FingerTapGesture;
import com.limelight.binding.input.touch.TwoFingerTapGesture;
import com.limelight.binding.input.touch.TouchMode;
import com.limelight.binding.input.touch.TouchMomentum;
import com.limelight.binding.input.touch.TrackpadArea;
import com.limelight.binding.input.touch.TrackpadPointer;
import com.limelight.binding.input.driver.UsbDriverService;
import com.limelight.binding.input.evdev.EvdevListener;
import com.limelight.binding.input.touch.TouchContext;
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
import com.limelight.nvstream.PyroWaveBitrateFloor;
import com.limelight.nvstream.RateChangeResult;
import com.limelight.nvstream.NvConnectionListener;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ClipboardImageException;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.HardwareMetrics;
import com.limelight.nvstream.http.LaunchConfirmation;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.input.ControllerPacket;
import com.limelight.nvstream.input.KeyboardPacket;
import com.limelight.nvstream.input.MouseButtonPacket;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.AppQuitOnLeave;
import com.limelight.preferences.AutoBitrateChoice;
import com.limelight.preferences.GlPreferences;
import com.limelight.preferences.HostDisplayChoice;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.GameGestures;
import com.limelight.ui.HostCursorShapes;
import com.limelight.ui.HostKeyboard;
import com.limelight.ui.HostTextField;
import com.limelight.ui.KeyboardOverlay;
import com.limelight.ui.LegacyKeyboardInsets;
import com.limelight.ui.PcKeyboardOverlay;
import com.limelight.binding.input.keyboard.PcKeyboardController;
import com.limelight.binding.input.keyboard.PcKeyboardEngine;
import com.limelight.ui.DesktopControlsPanel;
import com.limelight.ui.StreamOrientation;
import com.limelight.ui.StreamView;
import com.limelight.ui.VideoScaling;
import com.limelight.utils.ClipboardImages;
import com.limelight.utils.Dialog;
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
import android.view.KeyCharacterMap;
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
import android.view.WindowInsets;
import android.view.WindowManager;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.view.inputmethod.InputMethodManager;
import android.view.inputmethod.InputMethodSubtype;
import android.widget.TextView;
import android.widget.Toast;

import org.xmlpull.v1.XmlPullParserException;

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import com.limelight.ui.ActionSheet;
import java.util.HashMap;
import java.math.BigDecimal;
import java.util.Locale;
import java.util.Map;


public class Game extends Activity implements SurfaceHolder.Callback, TextureView.SurfaceTextureListener,
        OnGenericMotionListener, OnTouchListener, NvConnectionListener, EvdevListener,
        OnSystemUiVisibilityChangeListener, GameGestures, StreamView.InputCallbacks,
        PerfOverlayListener, UsbDriverService.UsbDriverStateListener, View.OnKeyListener,
        com.limelight.ui.StreamWindow {
    private int lastButtonState = 0;
    private final MouseDeltaAccumulator mouseDeltaX = new MouseDeltaAccumulator();
    private final MouseDeltaAccumulator mouseDeltaY = new MouseDeltaAccumulator();

    // Mouse emulation uses two touches; native touch forwards all pointers.
    private final TouchContext[] touchContextMap = new TouchContext[2];
    private final FingerTapGesture fingerTap = new FingerTapGesture();
    private final TwoFingerTapGesture twoFingerTap = new TwoFingerTapGesture();
    private TrackpadPointer trackpadPointer;
    private TrackpadArea trackpadArea;
    // Set while the host sends its cursor shapes for the Local cursor; read on the control thread.
    private volatile HostCursorShapes hostCursorShapes;
    // The current finger gesture started outside the trackpad area or while touch is off.
    private boolean touchGestureIgnored;
    private boolean middleButtonUpPending;
    private final Runnable middleButtonUp = () -> {
        middleButtonUpPending = false;
        Game.this.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_MIDDLE);
    };
    private boolean rightButtonUpPending;
    private final Runnable rightButtonUp = () -> {
        rightButtonUpPending = false;
        Game.this.conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_RIGHT);
    };
    private boolean nativeTouchEnabled;
    private boolean nativeTouchUnsupported;
    private boolean nativeTouchGestureActive;

    private static final int STYLUS_DOWN_DEAD_ZONE_DELAY = 100;
    private static final int STYLUS_DOWN_DEAD_ZONE_RADIUS = 20;

    private static final int STYLUS_UP_DEAD_ZONE_DELAY = 150;
    private static final int STYLUS_UP_DEAD_ZONE_RADIUS = 50;

    // Travel allowed for a three- or four-finger tap, in dp.
    private static final int FINGER_TAP_SLOP_DP = 40;

    private ControllerHandler controllerHandler;
    private KeyboardTranslator keyboardTranslator;
    private KeyboardOverlay keyboardOverlay;
    private PcKeyboardOverlay pcKeyboard;
    // Controller keys the on-screen PC keyboard took on their way down, so their release goes to it too.
    private final android.util.SparseBooleanArray pcKeyboardKeys = new android.util.SparseBooleanArray();
    // The pad whose stick or hat is steering the on-screen keyboard, or -1.
    private int pcKeyboardAxisDevice = -1;
    private final InputManager.InputDeviceListener pcKeyboardDeviceListener = new InputManager.InputDeviceListener() {
        // A keyboard plugged in or switched to another layout changes what the host should type.
        @Override public void onInputDeviceAdded(int deviceId) { reportKeyboardLayout(); }
        @Override public void onInputDeviceChanged(int deviceId) { reportKeyboardLayout(); }
        @Override public void onInputDeviceRemoved(int deviceId) { runOnUiThread(Game.this::releasePcKeyboardPads); }
    };
    private boolean desktopWindowFocused = true;
    private boolean dispatchingDesktopTouch;
    private int desktopHeldButtons;
    private MouseDeltaAccumulator desktopDeltaX = new MouseDeltaAccumulator();
    private MouseDeltaAccumulator desktopDeltaY = new MouseDeltaAccumulator();
    private MouseDeltaAccumulator desktopScrollV = new MouseDeltaAccumulator();
    private MouseDeltaAccumulator desktopScrollH = new MouseDeltaAccumulator();
    private VirtualController virtualController;
    // The modern on-screen controller, used instead of the classic one when the settings ask for it.
    private com.limelight.binding.input.pad.ModernController modernController;
    private com.limelight.ui.MicButton micButton;

    private PreferenceConfiguration prefConfig;
    private SharedPreferences tombstonePrefs;

    private NvConnection conn;
    private SpinnerDialog spinner;
    private boolean displayedFailureDialog = false;
    private boolean connecting = false;
    private volatile boolean connected = false;
    private boolean autoEnterPip = false;
    private com.limelight.ui.AdaptiveLayout adaptiveLayout;
    private boolean surfaceCreated = false;
    private boolean attemptedConnection = false;
    private volatile Thread connectionStopThread;
    private StreamRecoveryPolicy recoveryPolicy = new StreamRecoveryPolicy(0, 0);
    private long connectedAt = -1;
    private int recoveryAppId;
    private String recoveryAppUuid;
    private Runnable pendingRecovery;
    private int recoveryGeneration;
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
    private android.app.Dialog streamMenu;
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
    private boolean foreground;
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
    private int currentBitrate;
    private AdaptiveBitrateController adaptiveBitrate;
    private PyroWaveBitrateController pyroWaveBitrate;
    private boolean poorConnection;
    private long adaptiveBitrateRetryAt;
    private int adaptiveBitrateRetryTarget;
    // Claims of the host's encoder bitrate that failed in a row. Some hosts report 0 for good.
    private static final int MAX_BITRATE_CLAIM_FAILURES = 6;
    private int adaptiveBitrateClaimFailures;
    // Automatic quality switched auto bitrate on for itself; the person's own choice was off.
    private boolean qualityAutoBitrate;
    private AdaptiveQualityController adaptiveQuality;
    private AdaptiveQualityController.Target pendingQualityTarget;
    private long qualityRequestSentAt;
    private String qualityStatus;
    private int qualityMaxWidth, qualityMaxHeight, qualityMaxFps;
    private int qualityMaxFrameRateMilliHz;
    private final Handler bitrateHandler = new Handler(Looper.getMainLooper());
    private final com.limelight.utils.StreamHealth streamHealth = new com.limelight.utils.StreamHealth();
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
                    LimeLog.info("Stream health: the last sample is still running; this one is skipped");
                }
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
                        decoderRenderer.getThermalLevel(),
                        adaptiveBitrate != null && adaptiveBitrate.hasRoomBeforeQualityReduction(),
                        decoderRenderer.takeDecodeTiming(streamFps));
                if (target != null && !applyAdaptiveQuality(target, now)) return;
            }
            bitrateHandler.postDelayed(this, 1000);
        }
    };
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
                    int thermalLevel = decoderRenderer.getThermalLevel();
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
    private NvApp app;
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

    // Asks a host that announces text field notices (0x553B) to say when a text field gains or loses
    // focus, so the soft keyboard follows it (setting "Keyboard follows text fields"), and reports
    // the keyboard layout to a host that takes it (0x553E). The C engine supports neither.
    private void requestTextInput() {
        reportedKeyboardLayout = null;
        hostKeyboard.reset();
        updateKeyboardPan();
        streamView.setHostTextField(null);
        reportKeyboardLayout();
        if (conn == null || !conn.canSendInput() || !prefConfig.keyboardFollowsTextFields ||
                !MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_TEXT_INPUT)) {
            return;
        }
        if (!MoonBridge.sendTextInputRequest(true)) {
            LimeLog.warning("Could not ask the host for text field notices");
        }
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

    private void reportKeyboardLayout() {
        if (!connected || conn == null ||
                !MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_KEYBOARD_LAYOUT)) {
            return;
        }
        String tag = currentKeyboardLayout();
        if (tag == null || tag.equals(reportedKeyboardLayout)) return;
        if (MoonBridge.sendKeyboardLayout(tag)) {
            reportedKeyboardLayout = tag;
            LimeLog.info("Reported keyboard layout " + tag);
        } else {
            LimeLog.warning("Could not report keyboard layout " + tag);
        }
    }

    // Apps cannot read a hardware keyboard's layout; the IME's subtype says which layout it uses
    // for physical keyboards (Android 14+), else which language it types.
    private String currentKeyboardLayout() {
        InputMethodManager inputManager = (InputMethodManager) getSystemService(Context.INPUT_METHOD_SERVICE);
        InputMethodSubtype subtype = inputManager != null ? inputManager.getCurrentInputMethodSubtype() : null;
        if (subtype == null) return null;
        String hint = null;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            android.icu.util.ULocale locale = subtype.getPhysicalKeyboardHintLanguageTag();
            if (locale != null) hint = locale.toLanguageTag();
        }
        String language = Build.VERSION.SDK_INT >= Build.VERSION_CODES.N ? subtype.getLanguageTag() : null;
        return KeyboardLayoutTag.pick(hint, language, subtype.getLocale());
    }

    // An attached hardware keyboard types itself; the soft keyboard must not take its place.
    private boolean hasHardwareKeyboard() {
        Configuration config = getResources().getConfiguration();
        return config.keyboard == Configuration.KEYBOARD_QWERTY &&
                config.hardKeyboardHidden == Configuration.HARDKEYBOARDHIDDEN_NO;
    }

    private boolean isSystemKeyboardVisible() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            WindowInsets insets = streamView.getRootWindowInsets();
            return insets != null && insets.isVisible(WindowInsets.Type.ime());
        }
        if (legacyKeyboard != null) return legacyKeyboard.visible();
        WindowInsetsCompat insets = ViewCompat.getRootWindowInsets(streamView);
        return insets != null && insets.isVisible(WindowInsetsCompat.Type.ime());
    }

    // The host's text field notice, on the UI thread: a raise or an update (the focus moved to
    // another field) types the keyboard for the field, and opens it right after the user's own
    // tap, click or key on the stream unless they closed it for this field (the host's element id,
    // 0 from a host that sends none); a lower hides only a keyboard the host opened. The keyboard
    // the user opened stays open.
    private void onHostTextField(int kind, int inputType, int flags, int element) {
        if (isFinishing() || streamView == null) return;
        if (kind == HostTextField.KIND_LOWER) {
            LimeLog.info("Host text field lost focus");
            streamView.setHostTextField(null);
            if (hostKeyboard.onLower()) hideSystemKeyboard();
            return;
        }
        if (kind != HostTextField.KIND_RAISE && kind != HostTextField.KIND_UPDATE) return;
        LimeLog.info("Host text field focused: type " + inputType + ", flags " + flags +
                ", element " + Integer.toHexString(element));
        hostKeyboard.onFocus(element);
        reportKeyboardLayout();
        if (!HostTextField.mayShowKeyboard(foreground, connected, streamMenu != null,
                conn != null && conn.canSendInput(), isPcKeyboardVisible(), hasHardwareKeyboard())) {
            return;
        }
        boolean visible = isSystemKeyboardVisible();
        streamView.setHostTextField(new HostTextField(inputType, flags));
        if (visible) return;
        long now = SystemClock.uptimeMillis();
        if (!hostKeyboard.mayOpen(now)) {
            LimeLog.info(hostKeyboard.dismissedHere() ? "Keyboard stays closed: closed for this field" :
                    "Keyboard stays closed: no tap, click or key on the stream in the last " +
                            HostKeyboard.INPUT_WINDOW_MS + " ms");
            return;
        }
        hideKeyboardOverlay();
        setInputGrabState(true);
        streamView.requestFocus();
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            getWindow().getInsetsController().show(WindowInsets.Type.ime());
        } else {
            InputMethodManager inputManager = (InputMethodManager) getSystemService(Context.INPUT_METHOD_SERVICE);
            inputManager.showSoftInput(streamView, 0);
        }
        hostKeyboard.onOpened();
    }

    // The user's own tap or click on the stream (key presses note themselves in handleKeyDown): a
    // host text field raised right after it may open the keyboard, and the point it pressed stays
    // above the keyboard. A trackpad finger or a captured mouse moves the PC's pointer instead.
    private void noteStreamPress(View view, MotionEvent event) {
        streamView.onStreamPress(event);
        int action = event.getActionMasked();
        if (action != MotionEvent.ACTION_DOWN && action != MotionEvent.ACTION_POINTER_DOWN &&
                action != MotionEvent.ACTION_BUTTON_PRESS) {
            return;
        }
        boolean finger = event.isFromSource(InputDevice.SOURCE_TOUCHSCREEN) ||
                event.isFromSource(InputDevice.SOURCE_STYLUS);
        boolean points = finger ? !TouchMode.isRelative(touchMode()) :
                event.isFromSource(InputDevice.SOURCE_MOUSE) && !event.isFromSource(InputDevice.SOURCE_MOUSE_RELATIVE);
        float y = Float.NaN;
        if (points && streamView.getHeight() > 0) {
            y = (event.getY(event.getActionIndex()) + getStreamViewOffsetY(view)) / streamView.getHeight();
        }
        hostKeyboard.onPress(y, SystemClock.uptimeMillis());
    }

    // Every inset dispatch on the stream window. The window takes no room from the keyboard
    // (adjustNothing): the video surface keeps its size, and the stream moves up instead. Android
    // reports the keyboard's insets to such a window from 11 on; before, legacyKeyboard measures it.
    private void onWindowInsets(WindowInsetsCompat insets) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return;
        onKeyboardInsets(insets.isVisible(WindowInsetsCompat.Type.ime()),
                insets.getInsets(WindowInsetsCompat.Type.ime()).bottom);
    }

    // The keyboard showed, hid or changed height, covering imeBottom px of the content view's bottom.
    private void onKeyboardInsets(boolean visible, int imeBottom) {
        if (streamView == null) return;
        keyboardInsetBottom = imeBottom;
        if (hostKeyboard.onVisibility(visible, SystemClock.uptimeMillis())) {
            LimeLog.info(hostKeyboard.visible() ? "Soft keyboard shown" : "Soft keyboard hidden");
        }
        // Insets come before layout: measure the stream once it is laid out, once for a burst.
        keyboardPanHandler.removeCallbacks(keyboardPan);
        keyboardPanHandler.post(keyboardPan);
    }

    // The insets are the content view's, and the content view is the stream's parent.
    private void updateKeyboardPan() {
        if (streamView == null) return;
        float pan = 0;
        View parent = (View) streamView.getParent();
        if (parent != null) {
            pan = hostKeyboard.pan(parent.getHeight(), keyboardInsetBottom, streamView.getTop(), streamView.getHeight(),
                    HostKeyboard.MARGIN_DP * getResources().getDisplayMetrics().density);
        }
        if (pan == streamView.getTranslationY()) return;
        LimeLog.info(pan == 0 ? "Stream moves back down" : "Stream moves " + Math.round(-pan) + " px up for the soft keyboard");
        streamView.setTranslationY(pan);
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
                maxNits, maxAverageNits, minNits);
        if (sent != null) {
            LimeLog.info("Display luminance sent to host: " + sent);
        }
    }

    private void updateInputPollingRate() {
        Display display = getWindowManager().getDefaultDisplay();
        conn.setInputPollingRate(Build.VERSION.SDK_INT >= Build.VERSION_CODES.M ?
                display.getMode().getRefreshRate() : display.getRefreshRate());
    }
    private boolean useArr;
    private final DisplayRefreshDiagnostics displayRefreshDiagnostics = new DisplayRefreshDiagnostics();
    // The last successful vote made by this activity, not a measurement of the display's rate.
    private Float appSurfaceFrameRateVote;
    // Follows the content rate measured from the host's frame times (checkbox_content_rate_frame_rate).
    private final com.limelight.binding.video.DisplayFrameRatePolicy.ContentRateVote contentRateVote =
            new com.limelight.binding.video.DisplayFrameRatePolicy.ContentRateVote();
    // Whether the vote above allowed only seamless switches (the content rate vote, and every vote on API 30).
    private boolean appSurfaceSeamlessOnly;
    // Tells why the screen did not follow the vote (see RefreshSwitchDiagnosis); created once connected.
    private com.limelight.binding.video.RefreshSwitchMonitor refreshSwitchMonitor;
    private final Runnable pollRefreshSwitch = new Runnable() {
        @Override public void run() {
            if (!connected || isFinishing()) return;
            checkRefreshSwitch();
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
    private InputCaptureProvider inputCaptureProvider;
    private int modifierFlags = 0;
    private final Map<Short, Byte> pressedKeys = new HashMap<>();
    private boolean grabbedInput = true;
    private boolean cursorVisible = false;
    private boolean waitingForAllModifiersUp = false;
    private int specialKeyCode = KeyEvent.KEYCODE_UNKNOWN;
    private StreamView streamView;
    private StreamView renderView;
    private com.limelight.ui.StreamPresentation externalPresentation;
    private com.limelight.ui.DesktopGestures desktopGestures;
    private com.limelight.ui.GameTouchControls gameTouchControls;
    private boolean outputSwitchPending;
    private static final String EXTRA_OUTPUT_DISPLAY = "rubylight.outputDisplay";
    private static final String EXTRA_OUTPUT_RESUME_ONLY = "rubylight.outputResumeOnly";
    private final Map<Short, Integer> customPressedKeys = new HashMap<>();
    private Surface videoSurface;
    private long lastAbsTouchUpTime = 0;
    private long lastAbsTouchDownTime = 0;
    private float lastAbsTouchUpX, lastAbsTouchUpY;
    private float lastAbsTouchDownX, lastAbsTouchDownY;

    private boolean isHidingOverlays;
    private boolean wasInPictureInPicture;
    private TextView notificationOverlayView;
    private int requestedNotificationOverlayVisibility = View.GONE;
    private TextView performanceOverlayView;
    private View performanceOverlayScroll;
    private CharSequence debugPerformanceText = "";
    private CharSequence statisticsPerformanceText = "";
    private CharSequence compactPerformanceText = "";
    // What the host last said about holding the bitrate down for the network; null when it has not.
    private LinkNoticeStatus linkNotice;
    // Whether a host text field may open the soft keyboard, whose it is, and how far the stream
    // moves up for it.
    private final HostKeyboard hostKeyboard = new HostKeyboard();
    // How much of the window's bottom the soft keyboard covers, from the last inset dispatch.
    private int keyboardInsetBottom;
    // The soft keyboard below Android 11, which tells an adjustNothing window nothing of it.
    private LegacyKeyboardInsets legacyKeyboard;
    private final Handler keyboardPanHandler = new Handler(Looper.getMainLooper());
    private final Runnable keyboardPan = this::updateKeyboardPan;
    // The layout last reported to the host this connection, or null.
    private String reportedKeyboardLayout;
    // PerformanceOverlay.MODE_COMPACT, MODE_STATISTICS or MODE_DEBUG
    private String overlayMode = PerformanceOverlay.MODE_COMPACT;
    // The corner the overlay prefers; OverlayPlacement moves it when controls cover that corner.
    private com.limelight.ui.OverlayPlacement.Corner overlayCorner = com.limelight.ui.OverlayPlacement.Corner.TOP_LEFT;
    private TextView streamMenuHint;
    // Every message of the stream goes through here: one at a time, lasting facts once per PC.
    private final com.limelight.ui.StreamMessages messages = new com.limelight.ui.StreamMessages(this);
    // From the first time the user opens USB devices until the stream stops.
    private com.limelight.binding.usb.UsbForwarding usbForwarding;
    // Clipboard sync (0x5508) while this stream's PC runs it.
    private com.limelight.utils.ClipboardSync clipboardSync;
    private final Runnable showStreamMenuHint = this::showStreamMenuHint;
    private final Runnable expireStreamMenuHint = () -> dismissStreamMenuHint(true);
    private HardwareMetrics hardwareMetrics;
    private boolean hardwareMetricsPolling;
    private int hardwareMetricsGeneration;
    private long hardwareMetricsSampleTimestamp = Long.MIN_VALUE;
    private long hardwareMetricsFreshUntil;
    private final Runnable expireHardwareMetrics = () -> {
        hardwareMetrics = null;
        updatePerformanceOverlay();
    };

    private MediaCodecDecoderRenderer decoderRenderer;
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
        setInputGrabState(false);
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
    public static final String EXTRA_APP_HDR = "HDR";
    public static final String EXTRA_SERVER_CERT = "ServerCert";
    private static final String EXTRA_RECOVERY_ATTEMPTS = "RecoveryAttempts";
    // The network the stream started on (StreamLayers.Network), kept across reconnects and handoffs.
    private static final String EXTRA_STREAM_NETWORK = "StreamNetwork";
    private static final String EXTRA_RECOVERY_STARTED = "RecoveryStarted";
    private static final String EXTRA_RECOVERY_MEDIA_FAILURE = "RecoveryMediaFailure";
    // Packed full chroma failed in this session and the reconnect runs without it (not the setting).
    private static final String EXTRA_PACKED_OFF = "PackedChromaOff";
    private static final String EXTRA_MICROPHONE_MUTED = "rubylight.microphoneMuted";

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
        showStreamLayers(prefConfig, startNetwork == null);
        recoveryPolicy = new StreamRecoveryPolicy(getIntent().getIntExtra(EXTRA_RECOVERY_ATTEMPTS, 0),
                getIntent().getLongExtra(EXTRA_RECOVERY_STARTED, 0));
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
        adaptiveLayout.setInsetsObserver(this::onWindowInsets);
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            legacyKeyboard = new LegacyKeyboardInsets(this, findViewById(android.R.id.content), this::onKeyboardInsets);
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
        performanceOverlayScroll.setOnTouchListener((view, event) -> !isDebugOverlay() && onOverlayTouch(view, event));
        overlayMode = PerformanceOverlay.normalizeMode(PreferenceManager.getDefaultSharedPreferences(this)
                .getString("performance_overlay_mode", PerformanceOverlay.MODE_COMPACT));
        overlayCorner = com.limelight.ui.OverlayPlacement.Corner.parse(prefConfig.overlayPosition);
        attachOverlayPlacement((android.view.ViewGroup) streamView.getParent());
        updatePerformanceOverlay();

        inputCaptureProvider = InputCaptureManager.getInputCaptureProvider(this, this);

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            streamView.setOnCapturedPointerListener(new View.OnCapturedPointerListener() {
                @Override
                public boolean onCapturedPointer(View view, MotionEvent motionEvent) {
                    return handleMotionEvent(view, motionEvent);
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
            if (!willStreamHdr) refuseHdr(refusal);
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
                        // The MediaCodec instance is going down due to a crash
                        // let's tell the user something when they open the app again

                        // A stream that is ending, or a TV that just lost its display, is not a decoder
                        // crash: counting it would disable features and reset settings next time.
                        CrashAccounting.Verdict verdict = CrashAccounting.classify(streamEnding(),
                                isAndroidTv(), videoDisplayOff);
                        if (verdict != CrashAccounting.Verdict.COUNT) {
                            LimeLog.warning("Decoder failure not counted as a crash (" +
                                    (verdict == CrashAccounting.Verdict.DISPLAY_LOST ? "display lost" : "stream ending") +
                                    "): " + e);
                            return;
                        }

                        // We must use commit because the app will crash when we return from this function
                        synchronized (tombstonePrefs) {
                            if (reportedCrash) {
                                return;
                            }
                            reportedCrash = true;
                            tombstonePrefs.edit()
                                    .putInt("CrashCount", tombstonePrefs.getInt("CrashCount", 0) + 1)
                                    .putString("CrashHostUuid", getIntent().getStringExtra(EXTRA_PC_UUID))
                                    .commit();
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
                refuseHdr(refusal);
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
            hostNotice(HostNotices.Notice.CODEC_UNAVAILABLE, R.string.stream_hevc_decoder_missing);
        }

        // Display a message to the user if AV1 was forced on but we still didn't find a decoder
        if (mediaStream && prefConfig.videoFormat == PreferenceConfiguration.FormatOption.FORCE_AV1 && !decoderRenderer.isAv1Supported()) {
            hostNotice(HostNotices.Notice.CODEC_UNAVAILABLE, R.string.stream_av1_decoder_missing);
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
                .setVrr(!smallStream && prefConfig.vrr)
                .setClipboardSync(prefConfig.clipboardSync && prefConfig.rustEngine)
                .setHostDisplay(prefConfig.hostDisplay)
                .setAudioConfiguration(smallStream ? MoonBridge.AUDIO_CONFIGURATION_STEREO : prefConfig.audioConfiguration)
                .setColorSpace(decoderRenderer.getPreferredColorSpace())
                .setColorRange(decoderRenderer.getPreferredColorRange())
                .setPersistGamepadsAfterDisconnect(!prefConfig.multiController)
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

        initializeTouchContexts();

        desktopGestures = new com.limelight.ui.DesktopGestures(this, streamView,
                this::cancelHostTouchInput, this::clickRightMouseButton);
        trackpadPointer.setViewport(desktopGestures.viewport());
        if (app.getRole() != NvApp.Role.REMOTE_MONITOR) {
            String profile = com.limelight.binding.input.TouchLayout.profileKey(getIntent().getStringExtra(EXTRA_PC_UUID),
                    app.getAppUuid(), app.getAppId());
            gameTouchControls = new com.limelight.ui.GameTouchControls(this, profile,
                    new com.limelight.binding.input.HeldInputState.Sink() {
                        @Override public void key(int key, boolean down) { sendCustomKey(key, down); }
                        @Override public void mouse(int button, boolean down) { sendCustomMouse(button, down); }
                        @Override public void gamepad(int buttons, short leftX, short leftY, short rightX, short rightY,
                                                      byte leftTrigger, byte rightTrigger) {
                            if (controllerHandler != null) controllerHandler.reportLayoutGamepadState(buttons,
                                    leftX, leftY, rightX, rightY, leftTrigger, rightTrigger);
                        }
                    }, () -> foreground && connected && grabbedInput && streamMenu == null && conn.canSendInput(),
                    this::showStreamDialog);
            gameTouchControls.setStreamTouchListener(this::onTouchControlsPassthrough);
            gameTouchControls.setGestureCancelListener(this::cancelTouchInput);
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
            updateMicButton();
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

    private Display getVideoDisplay() {
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
            rememberStreamForResume();
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
        reportKeyboardLayout();

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
                updateMicButton();
                if (keyboardOverlay != null) {
                    hideKeyboardOverlay();
                }
                hidePcKeyboard();

                performanceOverlayScroll.setVisibility(View.GONE);
                notificationOverlayView.setVisibility(View.GONE);
                dismissStreamMenuHint(false);

                // Disable sensors while in PiP mode
                controllerHandler.disableSensors();

                if (trackpadPointer != null) {
                    trackpadPointer.stopMomentum();
                    trackpadPointer.setCursorShown(false);
                }

                // Update GameManager state to indicate we're in PiP (still gaming, but interruptible)
                decoderRenderer.notifyVideoBackground();
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
                updateMicButton();

                if (prefConfig.enablePerfOverlay && app.getRole() != NvApp.Role.INPUT_ONLY) {
                    performanceOverlayScroll.setVisibility(View.VISIBLE);
                }

                notificationOverlayView.setVisibility(requestedNotificationOverlayVisibility);

                // Enable sensors again after exiting PiP
                controllerHandler.enableSensors();

                // Update GameManager state to indicate we're out of PiP (gaming, non-interruptible)
                decoderRenderer.notifyVideoForeground();
                UiHelper.notifyStreamExitingPiP(this);
            }
        }
        updateHardwareMetricsPolling();
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
        // Android has native keyboard capture support starting in API 36.1
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA && Build.VERSION.SDK_INT_FULL >= Build.VERSION_CODES_FULL.BAKLAVA_1) {
            WindowManager.LayoutParams windowLayoutParams = getWindow().getAttributes();
            windowLayoutParams.setKeyboardCaptureEnabled(enabled);
            getWindow().setAttributes(windowLayoutParams);
        }
        else {
            // This uses custom APIs present on some Samsung devices to allow capture of
            // meta key events while streaming.
            try {
                Class<?> semWindowManager = Class.forName("com.samsung.android.view.SemWindowManager");
                Method getInstanceMethod = semWindowManager.getMethod("getInstance");
                Object manager = getInstanceMethod.invoke(null);

                if (manager != null) {
                    Class<?>[] parameterTypes = new Class<?>[2];
                    parameterTypes[0] = ComponentName.class;
                    parameterTypes[1] = boolean.class;
                    Method requestMetaKeyEventMethod = semWindowManager.getDeclaredMethod("requestMetaKeyEvent", parameterTypes);
                    requestMetaKeyEventMethod.invoke(manager, this.getComponentName(), enabled);
                }
                else {
                    LimeLog.warning("SemWindowManager.getInstance() returned null");
                }
            } catch (ClassNotFoundException ignored) {
            } catch (NoSuchMethodException e) {
                e.printStackTrace();
            } catch (InvocationTargetException e) {
                e.printStackTrace();
            } catch (IllegalAccessException e) {
                e.printStackTrace();
            }
        }
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
            cancelTouchInput();
            releaseKeyboardInput();
            releaseMouseButtons();
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

        // A split-screen or desktop window is still visible at full streaming quality.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && isInPictureInPictureMode()) {
            decoderRenderer.notifyVideoBackground();
        } else {
            decoderRenderer.notifyVideoForeground();
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
            cancelTouchInput();
            initializeTouchContexts();
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
        setInputGrabState(false);
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
            setInputGrabState(true);
        }
        restoreInputOnResume = false;
        updateHardwareMetricsPolling();
    }

    @Override
    protected void onPause() {
        releaseDesktopInput();
        if (trackpadPointer != null) trackpadPointer.stopMomentum();
        if (gameTouchControls != null) gameTouchControls.releaseInput();
        // Shown again on the next stream unless it was read.
        dismissStreamMenuHint(false);
        foreground = false;
        recoveryCancelledOnPause = recoveryHandoffInProgress || pendingRecovery != null || (isRecoveryAttempt() && connecting);
        cancelAutomaticRecovery();
        stopHardwareMetricsPolling();
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
            setInputGrabState(false);
        }

        super.onPause();
    }

    @Override
    protected void onStop() {
        super.onStop();
        if (adaptiveLayout != null) adaptiveLayout.stop();
        rememberStreamForResume();

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
        } else if (connected && isPcKeyboardVisible()) {
            hidePcKeyboard();
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

    private void showStreamDialog(android.app.Dialog dialog) {
        hidePcKeyboard();
        setInputGrabState(false);
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
                    setInputGrabState(restoreInputAfterMenu);
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

    public void showStreamMenu() {
        if (!foreground || !connected || streamMenu != null) {
            return;
        }
        restoreInputAfterMenu = grabbedInput;
        // Whoever opens the menu knows how; the first-stream hint has done its job.
        dismissStreamMenuHint(true);
        streamMenuSheet = new com.limelight.ui.StreamMenu(this, this::streamMenuOptions, this::onStreamMenuItem,
                nativeMenuController());
        showStreamDialog(streamMenuSheet.sheet());
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
        options.micButtonAvailable = micButton != null;
        options.micButtonShown = micButton != null && prefConfig.micButton;
        options.overlayShown = prefConfig.enablePerfOverlay;
        options.overlayMode = overlayMode;
        options.overlayPosition = overlayCorner.key;
        options.localOutput = externalPresentation == null;
        options.mouseConnected = hasExternalMouse();
        options.cursorVisible = cursorVisible;
        options.onscreenControls = prefConfig.onscreenController;
        options.gameControls = gameTouchControls != null;
        options.keyboardKeysShown = isKeyboardOverlayVisible();
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
            case OVERLAY: togglePerformanceOverlay(); return;
            case MICROPHONE: toggleMicrophoneMute(); return;
            default: break;
        }
        Runnable action = null;
        ComputerDetails details = conn.getHostDetails();
        switch (item) {
            case PC_KEYBOARD: action = this::togglePcKeyboard; break;
            case KEYBOARD: action = this::toggleKeyboard; break;
            case TOUCH_MODE: action = this::showTouchModeDialog; break;
            case MOUSE_CURSOR: action = this::toggleCursorVisibility; break;
            case ONSCREEN_CONTROLS: action = () -> setOnscreenControlsEnabled(!prefConfig.onscreenController); break;
            case CONTROLS_LAYOUT: action = this::showControllerLayoutDialog; break;
            case MIC_BUTTON: action = () -> setMicButtonShown(!prefConfig.micButton); break;
            case GAME_CONTROLS: action = gameTouchControls == null ? null : gameTouchControls::showMenu; break;
            case KEYBOARD_KEYS: action = this::toggleKeyboardOverlay; break;
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
            case OVERLAY_OFF: action = this::hidePerformanceOverlay; break;
            case OVERLAY_TOP_LEFT: action = () -> setPerformanceOverlayPosition(com.limelight.ui.OverlayPlacement.Corner.TOP_LEFT); break;
            case OVERLAY_TOP_RIGHT: action = () -> setPerformanceOverlayPosition(com.limelight.ui.OverlayPlacement.Corner.TOP_RIGHT); break;
            case OVERLAY_BOTTOM_LEFT: action = () -> setPerformanceOverlayPosition(com.limelight.ui.OverlayPlacement.Corner.BOTTOM_LEFT); break;
            case OVERLAY_BOTTOM_RIGHT: action = () -> setPerformanceOverlayPosition(com.limelight.ui.OverlayPlacement.Corner.BOTTOM_RIGHT); break;
            case OVERLAY_COMPACT: action = () -> showPerformanceOverlay(PerformanceOverlay.MODE_COMPACT); break;
            case OVERLAY_STATISTICS: action = () -> showPerformanceOverlay(PerformanceOverlay.MODE_STATISTICS); break;
            case OVERLAY_DEBUG: action = () -> showPerformanceOverlay(PerformanceOverlay.MODE_DEBUG); break;
            case OVERLAY_COPY: action = this::copyPerformanceStats; break;
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

    private void showPerformanceOverlay(String mode) {
        setPerformanceOverlayMode(mode);
        if (!prefConfig.enablePerfOverlay) togglePerformanceOverlay();
    }

    // Off from the menu: the same switch the settings' Off writes, the mode kept for the next time.
    private void hidePerformanceOverlay() {
        if (prefConfig.enablePerfOverlay) togglePerformanceOverlay();
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

    // A USB or Bluetooth mouse, not a touchscreen or a virtual device.
    private boolean hasExternalMouse() {
        for (int id : InputDevice.getDeviceIds()) {
            InputDevice device = InputDevice.getDevice(id);
            if (device == null || device.isVirtual() ||
                    (device.getSources() & InputDevice.SOURCE_MOUSE) != InputDevice.SOURCE_MOUSE) continue;
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q || device.isExternal()) return true;
        }
        return false;
    }

    // The same as Ctrl+Alt+Shift+C: show or hide this device's cursor, capturing input if needed.
    private void toggleCursorVisibility() {
        if (inputCaptureProvider == null) return;
        if (!grabbedInput) {
            setInputGrabState(true);
        }
        cursorVisible = !cursorVisible;
        if (cursorVisible || isKeyboardOverlayVisible() || isPcKeyboardVisible()) {
            inputCaptureProvider.showCursor();
        } else {
            inputCaptureProvider.hideCursor();
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

    // Once, on the first stream: how to open the stream menu with this device's own inputs.
    private void showStreamMenuHint() {
        if (!foreground || !connected || isFinishing() || streamMenu != null || streamMenuHint == null ||
                isHidingOverlays || com.limelight.ui.StreamMenuHint.seen(this)) {
            return;
        }
        boolean controller = com.limelight.ui.StreamMenuHint.hasGamepad() ||
                (controllerHandler != null && controllerHandler.hasUsbControllers());
        String text = com.limelight.ui.StreamMenuHint.text(this, controller);
        streamMenuHint.setText(text);
        streamMenuHint.setContentDescription(text + " " + getString(R.string.stream_menu_hint_close));
        streamMenuHint.setOnClickListener(view -> dismissStreamMenuHint(true));
        streamMenuHint.setVisibility(View.VISIBLE);
        com.limelight.ui.StreamMenuHint.place(streamMenuHint, performanceOverlayScroll);
        bitrateHandler.removeCallbacks(expireStreamMenuHint);
        bitrateHandler.postDelayed(expireStreamMenuHint, com.limelight.ui.StreamMenuHint.SHOW_MS);
    }

    private void dismissStreamMenuHint(boolean read) {
        bitrateHandler.removeCallbacks(showStreamMenuHint);
        bitrateHandler.removeCallbacks(expireStreamMenuHint);
        if (streamMenuHint != null) streamMenuHint.setVisibility(View.GONE);
        if (read) com.limelight.ui.StreamMenuHint.markSeen(this);
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

    private String wifiText(long stallPeriodMs) {
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
     * Says which profile and own values this stream started with, and loudly what could not be
     * used. A profile picked for the network is announced; the overlay always lists the layers.
     */
    private void showStreamLayers(PreferenceConfiguration config, boolean firstStart) {
        com.limelight.preferences.StreamLayers.Result layers = config.layers;
        if (layers == null) return;
        for (String problem : com.limelight.preferences.StreamLayersText.problems(this, layers)) {
            messages.alert(problem, Toast.LENGTH_LONG);
        }
        config.layersText = com.limelight.preferences.StreamLayersText.applied(this, layers);
        if (config.layersText == null) return;
        if (firstStart && layers.reason == com.limelight.preferences.StreamLayers.Reason.NETWORK) {
            messages.countedNotice(config.layersText);
        } else {
            messages.note(config.layersText);
        }
    }

    /**
     * Leaves the stream as the player asked. A game set to quit on leave is quit first, while the
     * stream still holds the host connection; a failure is reported because the game stays running.
     */
    private void leaveStream() {
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

    private void togglePerformanceOverlay() {
        if (app.getRole() == NvApp.Role.INPUT_ONLY) {
            return;
        }
        prefConfig.enablePerfOverlay = !prefConfig.enablePerfOverlay;
        performanceOverlayScroll.setVisibility(prefConfig.enablePerfOverlay && !isHidingOverlays ? View.VISIBLE : View.GONE);
        PreferenceManager.getDefaultSharedPreferences(this).edit()
                .putBoolean("checkbox_enable_perf_overlay", prefConfig.enablePerfOverlay).apply();
        updateHardwareMetricsPolling();
    }

    private boolean isDebugOverlay() {
        return PerformanceOverlay.MODE_DEBUG.equals(overlayMode);
    }

    private void setPerformanceOverlayMode(String mode) {
        overlayMode = PerformanceOverlay.normalizeMode(mode);
        PreferenceManager.getDefaultSharedPreferences(this).edit()
                .putString("performance_overlay_mode", overlayMode).apply();
        performanceOverlayScroll.scrollTo(0, 0);
        updatePerformanceOverlay();
        // The compact line leaves the host's hardware readings out; the other modes poll them.
        updateHardwareMetricsPolling();
    }

    private void setPerformanceOverlayPosition(com.limelight.ui.OverlayPlacement.Corner corner) {
        overlayCorner = corner;
        prefConfig.overlayPosition = corner.key;
        PreferenceManager.getDefaultSharedPreferences(this).edit()
                .putString("performance_overlay_position", corner.key).apply();
        // Move it now; a hidden overlay is placed by the next layout once it shows.
        android.view.ViewGroup parent = (android.view.ViewGroup) performanceOverlayScroll.getParent();
        int margin = Math.round(8 * getResources().getDisplayMetrics().density);
        com.limelight.ui.OverlayPlacement.Spot spot = com.limelight.ui.OverlayPlacement.place(
                performanceOverlayScroll, parent, this::collectOverlayObstacles, margin, corner);
        if (spot != null) alignOverlayText(spot);
    }

    private void attachOverlayPlacement(android.view.ViewGroup parent) {
        com.limelight.ui.OverlayPlacement.attach(performanceOverlayScroll, parent,
                this::collectOverlayObstacles, () -> overlayCorner, this::alignOverlayText);
    }

    // Lines of different widths hug the side the overlay sits on. Set only when the side changes.
    private void alignOverlayText(com.limelight.ui.OverlayPlacement.Spot spot) {
        int side = spot == com.limelight.ui.OverlayPlacement.Spot.TOP_END ||
                spot == com.limelight.ui.OverlayPlacement.Spot.BOTTOM_END ? android.view.Gravity.END : android.view.Gravity.START;
        if ((performanceOverlayView.getGravity() & android.view.Gravity.RELATIVE_HORIZONTAL_GRAVITY_MASK) != side) {
            performanceOverlayView.setGravity(android.view.Gravity.TOP | side);
        }
    }

    private void updatePerformanceOverlay() {
        boolean compact = PerformanceOverlay.MODE_COMPACT.equals(overlayMode);
        performanceOverlayView.setTextSize(compact ? 13 : isDebugOverlay() ? 11 : 12);
        CharSequence text = compact ? compactPerformanceText : hardwarePerformanceText(isDebugOverlay());
        if (PerformanceOverlay.MODE_STATISTICS.equals(overlayMode) && text.length() != 0) {
            text = PerformanceOverlay.withStatusLine(text, messages.notes());
        }
        if (text.length() == 0) {
            text = compact ? "● " + getString(R.string.overlay_compact_waiting) : getString(R.string.stats_waiting);
        }
        text = PerformanceOverlay.withLinkNotice(this, text, linkNotice);
        performanceOverlayView.setText(text);
        performanceOverlayView.setContentDescription(compact ?
                PerformanceOverlay.describeCompact(this, compactPerformanceText, linkNotice) : null);
    }

    // The on-screen gamepad, its layout button, per-game buttons and the key toolbar stay uncovered.
    private void collectOverlayObstacles(java.util.List<Rect> out) {
        View parent = (View) performanceOverlayScroll.getParent();
        if (virtualController != null && prefConfig.onscreenController) virtualController.collectVisibleBounds(out);
        if (modernController != null && prefConfig.onscreenController) modernController.collectVisibleBounds(out);
        if (micButton != null) com.limelight.ui.OverlayPlacement.addVisible(micButton, parent, out);
        if (gameTouchControls != null) gameTouchControls.collectVisibleBounds(parent, out);
        if (isKeyboardOverlayVisible()) com.limelight.ui.OverlayPlacement.addVisible(keyboardOverlay, parent, out);
        if (isPcKeyboardVisible()) com.limelight.ui.OverlayPlacement.addVisible(pcKeyboard, parent, out);
        // The poor-connection warning sits top right: the overlay keeps out of its way.
        com.limelight.ui.OverlayPlacement.addVisible(notificationOverlayView, parent, out);
    }

    private CharSequence hardwarePerformanceText(boolean debug) {
        HardwareMetrics fresh = SystemClock.uptimeMillis() < hardwareMetricsFreshUntil ? hardwareMetrics : null;
        return PerformanceOverlay.withHardwareMetrics(this, debug ? debugPerformanceText : statisticsPerformanceText, fresh, debug);
    }

    private void updateHardwareMetricsPolling() {
        if (!foreground || !connected || conn == null || isFinishing() || isDestroyed() || isHidingOverlays ||
                performanceOverlayScroll == null || performanceOverlayScroll.getVisibility() != View.VISIBLE ||
                PerformanceOverlay.MODE_COMPACT.equals(overlayMode)) {
            stopHardwareMetricsPolling();
            return;
        }
        if (hardwareMetricsPolling) return;
        int generation = ++hardwareMetricsGeneration;
        hardwareMetricsPolling = conn.startHardwareMetricsPolling(metrics -> bitrateHandler.post(() ->
                receiveHardwareMetrics(generation, metrics)));
    }

    private void receiveHardwareMetrics(int generation, HardwareMetrics metrics) {
        if (!hardwareMetricsPolling || generation != hardwareMetricsGeneration) return;
        long now = SystemClock.uptimeMillis();
        if (metrics != null) {
            if (metrics.sampledAtUnixMs != hardwareMetricsSampleTimestamp) {
                hardwareMetricsSampleTimestamp = metrics.sampledAtUnixMs;
                hardwareMetricsFreshUntil = now + Math.max(5000, metrics.intervalMs * 2L);
            }
            if (now >= hardwareMetricsFreshUntil) metrics = null;
        }
        hardwareMetrics = metrics;
        bitrateHandler.removeCallbacks(expireHardwareMetrics);
        if (metrics != null) bitrateHandler.postAtTime(expireHardwareMetrics, hardwareMetricsFreshUntil);
        updatePerformanceOverlay();
    }

    private void stopHardwareMetricsPolling() {
        hardwareMetricsGeneration++;
        if (hardwareMetricsPolling && conn != null) conn.stopHardwareMetricsPolling();
        hardwareMetricsPolling = false;
        bitrateHandler.removeCallbacks(expireHardwareMetrics);
        if (hardwareMetrics != null) {
            hardwareMetrics = null;
            updatePerformanceOverlay();
        }
    }

    private void copyPerformanceStats() {
        CharSequence text = hardwarePerformanceText(true);
        if (text.length() == 0) {
            messages.alert(R.string.overlay_empty, Toast.LENGTH_LONG);
            return;
        }
        ClipboardManager clipboard = (ClipboardManager) getSystemService(CLIPBOARD_SERVICE);
        clipboard.setPrimaryClip(ClipData.newPlainText(getString(R.string.overlay_copy),
                text));
        messages.alert(R.string.overlay_copied, Toast.LENGTH_SHORT);
    }

    private void initializeTouchContexts() {
        if (trackpadPointer == null) {
            trackpadPointer = new TrackpadPointer(conn, streamView, findViewById(R.id.localCursor),
                    prefConfig.absoluteMouseMode,
                    prefConfig.trackpadAcceleration ? PointerAcceleration.forCurve(prefConfig.trackpadAccelerationCurve) : null,
                    prefConfig.trackpadMomentum ? new TouchMomentum(prefConfig.trackpadMomentumGlide) : null);
            trackpadArea = TrackpadArea.forPreset(prefConfig.trackpadArea);
            if (desktopGestures != null) trackpadPointer.setViewport(desktopGestures.viewport());
        }
        trackpadPointer.stopMomentum();
        trackpadPointer.setReferenceSize(prefConfig.width, prefConfig.height);
        trackpadPointer.setLocalCursor(touchMode() == TouchMode.LOCAL_CURSOR);
        updateHostCursorShapes();
        for (int i = 0; i < touchContextMap.length; i++) {
            touchContextMap[i] = prefConfig.touchscreenTrackpad ?
                    new RelativeTouchContext(conn, i, prefConfig.width, prefConfig.height, streamView, prefConfig,
                            trackpadPointer) :
                    new AbsoluteTouchContext(conn, i, streamView,
                            () -> desktopGestures != null ? desktopGestures.viewport().scale() : 1);
        }
    }

    /**
     * Asks a host that offers cursor shapes for them while the Local cursor is in use, and stops
     * them when it is not. Without host support the Local cursor stays Nightjar's arrow.
     */
    private void updateHostCursorShapes() {
        ComputerDetails details = conn != null ? conn.getHostDetails() : null;
        boolean wanted = connected && trackpadPointer != null && trackpadPointer.isLocalCursor() && details != null &&
                details.caps.has(HostCapabilities.Feature.CURSOR_SHAPE) &&
                MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_CURSOR_SHAPE_REQUEST);
        HostCursorShapes current = hostCursorShapes;
        if (wanted == (current != null)) return;
        if (wanted) {
            HostCursorShapes shapes = new HostCursorShapes(getResources(), findViewById(R.id.localCursor), trackpadPointer,
                    () -> messages.alert(R.string.stream_cursor_shape_rejected, Toast.LENGTH_LONG));
            hostCursorShapes = shapes;
            if (MoonBridge.sendCursorShapeRequest(true)) {
                LimeLog.info("Host cursor shapes requested");
                return;
            }
            LimeLog.warning("Host cursor shape request could not be sent; keeping the local arrow");
            hostCursorShapes = null;
            shapes.close();
        } else {
            hostCursorShapes = null;
            MoonBridge.sendCursorShapeRequest(false);
            current.close();
        }
    }

    private void stopHostCursorShapes() {
        HostCursorShapes shapes = hostCursorShapes;
        if (shapes == null) return;
        hostCursorShapes = null;
        runOnUiThread(shapes::close);
    }

    /** The finger touch mode in effect; external displays and tests change the switches directly. */
    private int touchMode() {
        if (!prefConfig.touchscreenTrackpad) {
            return nativeTouchEnabled ? TouchMode.MULTI_TOUCH : TouchMode.DIRECT;
        }
        return prefConfig.touchMode == TouchMode.LOCAL_CURSOR || prefConfig.touchMode == TouchMode.OFF ?
                prefConfig.touchMode : TouchMode.TRACKPAD;
    }

    private boolean isInTrackpadArea(View view, MotionEvent event) {
        if (trackpadArea == null || trackpadArea.isFull()) return true;
        View parent = (View) streamView.getParent();
        if (parent == null) return true;
        float x = event.getX(0) + (view != null ? view.getX() : 0);
        float y = event.getY(0) + (view != null ? view.getY() : 0);
        return trackpadArea.contains(x, y, parent.getWidth(), parent.getHeight());
    }

    private void onFingerTap(String action) {
        if (FingerTapGesture.NONE.equals(action)) return;
        if (FingerTapGesture.DESKTOP_CONTROLS.equals(action)) {
            toggleKeyboardOverlay();
        } else if (FingerTapGesture.PC_KEYBOARD.equals(action)) {
            togglePcKeyboard();
        } else if (FingerTapGesture.MIDDLE_CLICK.equals(action)) {
            if (middleButtonUpPending) {
                bitrateHandler.removeCallbacks(middleButtonUp);
                middleButtonUp.run();
            }
            conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_MIDDLE);
            // Held briefly, like a trackpad tap, so games that poll the buttons see it.
            middleButtonUpPending = true;
            bitrateHandler.postDelayed(middleButtonUp, 100);
        } else {
            toggleKeyboard();
        }
    }

    /** Own the whole multi-finger sequence so lifting in any order cannot leak mouse events. */
    private boolean handleFingerTapGesture(MotionEvent event) {
        int action = event.getActionMasked();
        if (action == MotionEvent.ACTION_DOWN) fingerTap.reset();
        if (!fingerTap.isStarted() && action == MotionEvent.ACTION_POINTER_DOWN &&
                event.getPointerCount() == 3 &&
                (!FingerTapGesture.NONE.equals(prefConfig.threeFingerTap) ||
                        !FingerTapGesture.NONE.equals(prefConfig.fourFingerTap))) {
            fingerTap.start(event.getEventTime(), event.getPointerId(0), event.getX(0), event.getY(0));
            for (TouchContext context : touchContextMap) {
                if (context != null) {
                    context.cancelTouch();
                    context.setPointerCount(0);
                }
            }
            if (trackpadPointer != null) trackpadPointer.stopMomentum();
        }
        if (!fingerTap.isStarted()) return false;

        fingerTap.pointers(event.getPointerCount());
        int tracked = event.findPointerIndex(fingerTap.pointerId());
        if (tracked >= 0) {
            for (int i = 0; i < event.getHistorySize(); i++) {
                fingerTap.move(event.getHistoricalX(tracked, i), event.getHistoricalY(tracked, i));
            }
            fingerTap.move(event.getX(tracked), event.getY(tracked));
        }
        if (action == MotionEvent.ACTION_CANCEL) {
            fingerTap.reset();
        } else {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
                    (event.getFlags() & MotionEvent.FLAG_CANCELED) != 0) {
                fingerTap.invalidate();
            }
            if (action == MotionEvent.ACTION_UP) {
                int fingers = fingerTap.finish(event.getEventTime(),
                        FINGER_TAP_SLOP_DP * getResources().getDisplayMetrics().density);
                if (fingers == 3) onFingerTap(prefConfig.threeFingerTap);
                else if (fingers == 4) onFingerTap(prefConfig.fourFingerTap);
            }
        }
        return true;
    }

    private void clickRightMouseButton() {
        if (rightButtonUpPending) {
            bitrateHandler.removeCallbacks(rightButtonUp);
            rightButtonUp.run();
        }
        conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_RIGHT);
        rightButtonUpPending = true;
        bitrateHandler.postDelayed(rightButtonUp, 100);
    }

    private void cancelTouchInput() {
        if (desktopGestures != null) desktopGestures.cancelTouch();
        cancelHostTouchInput();
    }

    // Desktop pan/zoom takes over the host gesture while retaining its own tap candidate.
    private void cancelHostTouchInput() {
        for (TouchContext context : touchContextMap) {
            if (context != null) {
                context.cancelTouch();
                context.setPointerCount(0);
            }
        }
        if (nativeTouchGestureActive) {
            conn.sendTouchEvent(MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL, 0, 0, 0, 0, 0, 0, MoonBridge.LI_ROT_UNKNOWN);
            nativeTouchGestureActive = false;
        }
        fingerTap.reset();
        twoFingerTap.reset();
        touchGestureIgnored = false;
        if (trackpadPointer != null) trackpadPointer.stopMomentum();
        if (middleButtonUpPending) {
            bitrateHandler.removeCallbacks(middleButtonUp);
            middleButtonUp.run();
        }
        if (rightButtonUpPending) {
            bitrateHandler.removeCallbacks(rightButtonUp);
            rightButtonUp.run();
        }
    }

    private void showTouchModeDialog() {
        if (externalPresentation != null) {
            messages.alert(R.string.desktop_touchpad_help, Toast.LENGTH_LONG);
            return;
        }
        String[] modes = getResources().getStringArray(R.array.stream_touch_modes);
        showStreamDialog(new NavigationDialogBuilder(this).setTitle(R.string.stream_touch_mode)
                .setSingleChoiceItems(modes, touchMode(), (dialog, which) -> {
                    cancelTouchInput();
                    prefConfig.touchMode = which;
                    prefConfig.touchscreenTrackpad = TouchMode.legacyTrackpad(which);
                    nativeTouchEnabled = which == TouchMode.MULTI_TOUCH;
                    nativeTouchUnsupported = false;
                    initializeTouchContexts();
                    SharedPreferences.Editor editor = PreferenceManager.getDefaultSharedPreferences(this).edit();
                    TouchMode.save(editor, which);
                    editor.apply();
                    dialog.dismiss();
                    if (nativeTouchEnabled) {
                        messages.alert(R.string.stream_native_touch_help, Toast.LENGTH_LONG);
                    } else if (which == TouchMode.OFF) {
                        messages.alert(R.string.stream_touch_off_help, Toast.LENGTH_LONG);
                    }
                }).setNegativeButton(android.R.string.cancel, null).create());
    }

    private void setOnscreenControlsEnabled(boolean enabled) {
        if (!conn.canSendInput()) {
            return;
        }
        prefConfig.onscreenController = enabled;
        if (enabled) {
            showOnscreenController();
        } else {
            if (virtualController != null) {
                virtualController.setControllerMode(VirtualController.ControllerMode.Active);
                virtualController.hide();
            }
            if (modernController != null) {
                modernController.finishEditing(false);
                modernController.hide();
            }
        }
        // The microphone button's default place follows the modern controller's top row.
        if (micButton != null) micButton.place();
        PreferenceManager.getDefaultSharedPreferences(this).edit()
                .putBoolean("checkbox_show_onscreen_controls", enabled).apply();
        // Hiding controls changes no layout by itself; ask for one so the overlay can move back.
        performanceOverlayScroll.requestLayout();
    }

    // Creates the on-screen controller the settings ask for, or brings the existing one back.
    private void showOnscreenController() {
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

    // Shows the microphone button when its setting is on, the host takes the microphone and input is allowed.
    private void updateMicButton() {
        if (micButton == null) return;
        micButton.setSuppressed(!com.limelight.ui.MicButton.allowed(prefConfig.micButton, isHidingOverlays,
                conn != null && conn.canSendInput()));
        micButton.place();
    }

    private void setMicButtonShown(boolean shown) {
        prefConfig.micButton = shown;
        PreferenceManager.getDefaultSharedPreferences(this).edit()
                .putBoolean(com.limelight.preferences.PreferenceConfiguration.MIC_BUTTON_PREF_STRING, shown).apply();
        updateMicButton();
        performanceOverlayScroll.requestLayout();
    }

    private void showControllerLayoutDialog() {
        if (modernController != null) {
            // The modern controller edits in place: drag, pinch and the panel instead of modes.
            modernController.startEditing();
            return;
        }
        showStreamDialog(new NavigationDialogBuilder(this).setTitle(R.string.stream_controls_layout)
                .setItems(R.array.stream_control_layout_actions, (dialog, which) -> {
                    VirtualController.ControllerMode[] modes = {
                            VirtualController.ControllerMode.MoveButtons,
                            VirtualController.ControllerMode.ResizeButtons,
                            VirtualController.ControllerMode.Active};
                    virtualController.setControllerMode(modes[which]);
                }).setNegativeButton(android.R.string.cancel, null).create());
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
        return LaunchSource.put(new Intent(getIntent()), LaunchSource.again(why, getIntent()));
    }

    private Intent createReconnectIntent() {
        Intent retry = createStreamHandoffIntent(LaunchSource.RECONNECT);
        retry.removeExtra(EXTRA_RECOVERY_ATTEMPTS);
        retry.removeExtra(EXTRA_RECOVERY_STARTED);
        retry.removeExtra(EXTRA_RECOVERY_MEDIA_FAILURE);
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
        setInputGrabState(false);
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
        return getIntent().getIntExtra(EXTRA_RECOVERY_ATTEMPTS, 0) > 0;
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

    private void rememberStreamForResume() {
        if (stoppedStreamResumeIntent != null || prefConfig == null || !prefConfig.autoReconnect ||
                !connected || displayedFailureDialog || isFinishing() || isDestroyed() ||
                recoveryCancelledOnPause || recoveryHandoffInProgress || recoveryAppId == 0 ||
                app == null || app.getRole() != NvApp.Role.STREAM || app.isControlAction()) return;
        // Count only time actually connected, never time spent locked or in another app.
        recoveryPolicy.resetAfterStableConnection(SystemClock.elapsedRealtime(), connectedAt);
        connectedAt = -1;
        stoppedStreamResumeIntent = createStreamHandoffIntent(LaunchSource.RETURN)
                .putExtra(EXTRA_APP_ID, recoveryAppId).putExtra(EXTRA_APP_UUID, recoveryAppUuid);
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
        if (!queueAutomaticRecovery(retry)) {
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
        return queueAutomaticRecovery(retry, packedOff);
    }

    private boolean queueAutomaticRecovery(final Intent retry) {
        return queueAutomaticRecovery(retry, false);
    }

    private boolean queueAutomaticRecovery(final Intent retry, boolean ignoreAutoReconnectSetting) {
        if (prefConfig == null || (!ignoreAutoReconnectSetting && !prefConfig.autoReconnect) || !foreground || isFinishing() || isDestroyed() ||
                recoveryHandoffInProgress || pendingRecovery != null) return false;
        long delay = recoveryPolicy.nextDelay(SystemClock.elapsedRealtime(), connectedAt);
        if (delay < 0) {
            recoveryBudgetExhausted = true;
            return false;
        }

        displayedFailureDialog = true;
        setInputGrabState(false);
        stopConnection();
        if (spinner != null) spinner.dismiss();
        spinner = SpinnerDialog.displayDialog(this, getString(R.string.conn_establishing_title),
                getString(R.string.stream_recovery_attempt, recoveryPolicy.attempts(), StreamRecoveryPolicy.MAX_ATTEMPTS), true);
        retry.putExtra(EXTRA_RECOVERY_ATTEMPTS, recoveryPolicy.attempts())
                .putExtra(EXTRA_RECOVERY_STARTED, recoveryPolicy.startedAt())
                .putExtra(EXTRA_MICROPHONE_MUTED, getIntent().getBooleanExtra(EXTRA_MICROPHONE_MUTED, false));
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
                } catch (InterruptedException interrupted) {
                    Thread.currentThread().interrupt();
                    return;
                }
                runOnUiThread(() -> {
                    if (!foreground || generation != foregroundGeneration || recovery != recoveryGeneration || isFinishing() || isDestroyed()) return;
                    if ((stopping != null && stopping.isAlive()) ||
                            SystemClock.elapsedRealtime() - recoveryPolicy.startedAt() >= StreamRecoveryPolicy.WINDOW_MS) {
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
                showClipboardImageResult(ClipboardImages.message(e.reason), generation);
            } catch (OutOfMemoryError e) {
                LimeLog.warning("Clipboard image out of memory: " + e);
                showClipboardImageResult(R.string.clipboard_image_too_large, generation);
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
        field.setPadding(padding, 0, padding, 0);
        EditText input = new com.google.android.material.textfield.TextInputEditText(this);
        input.setMinHeight(Math.round(48 * getResources().getDisplayMetrics().density));
        input.setSingleLine(true);
        field.addView(input);
        input.setInputType(InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_FLAG_DECIMAL);
        input.setText(BigDecimal.valueOf(currentBitrate == 0 ? prefConfig.bitrate : currentBitrate, 3)
                .stripTrailingZeros().toPlainString());
        input.selectAll();
        AlertDialog bitrateDialog = new NavigationDialogBuilder(this).setTitle(R.string.stream_bitrate)
                .setMessage(R.string.stream_bitrate_help).setView(field)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(android.R.string.ok, null).create();
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
                        if (foreground && generation == foregroundGeneration && connected && !isFinishing() &&
                                !warnIfBelowPyroWaveFloor(applied)) {
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
        showStreamDialog(dialog);
        dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(view -> {
            if (!connected || rateChanges != stream) {
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
            boolean refused = result.bitrateStatus == RateChangeResult.STATUS_REFUSED ||
                    result.fpsStatus == RateChangeResult.STATUS_REFUSED;
            // A bitrate below PyroWave's floor says so instead, unless something was refused.
            if (refused || !result.bitrateChanged() || !outcome.takesBitrate ||
                    !warnIfBelowPyroWaveFloor(result.bitrateKbps)) {
                messages.alert(rateChangeText(this, result), Toast.LENGTH_LONG);
            }
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
        }
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
        rateChanges.startAutomatic(currentBitrate == 0 ? prefConfig.bitrate : currentBitrate, SystemClock.uptimeMillis());
        bitrateHandler.postDelayed(updateRateChangeBitrate, 1000);
    }

    // PyroWave is intra-only, so below its floor every frame breaks up. True when it was said.
    private boolean warnIfBelowPyroWaveFloor(int kbps) {
        if (prefConfig.disableWarnings || kbps <= 0 || decoderRenderer == null) return false;
        int format = decoderRenderer.getActiveVideoFormat();
        if ((format & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) == 0) return false;
        int[] size = decoderRenderer.getStreamSize();
        int floor = PyroWaveBitrateFloor.minimumKbps(size[0], size[1], streamFps,
                (format & MoonBridge.VIDEO_FORMAT_MASK_YUV444) != 0, (format & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0);
        if (kbps >= floor) return false;
        String text = getString(R.string.stream_pyrowave_bitrate_low, kbps / 1000.0, floor / 1000,
                size[0], size[1], streamFps);
        messages.alert(text, Toast.LENGTH_LONG);
        return true;
    }

    private boolean supportsAdaptiveBitrate() {
        ComputerDetails details = conn.getHostDetails();
        return app.getRole() != NvApp.Role.INPUT_ONLY && details.caps.has(HostCapabilities.Feature.RUNTIME_BITRATE) &&
                details.encoderBitrateSupported &&
                details.hasPermission(ComputerDetails.PERMISSION_VIEW | ComputerDetails.PERMISSION_LAUNCH);
    }

    private AdaptiveQualityController.Mode qualityMode() {
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

    private boolean isAdaptiveBitrateEnabled() {
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

    private void setInputGrabState(boolean grab) {
        if (inputCaptureProvider == null) {
            return;
        }
        grab = grab && conn != null && conn.canSendInput();
        // Grab/ungrab the mouse cursor
        if (grab) {
            inputCaptureProvider.enableCapture();

            // Enabling capture may hide the cursor again, so
            // we will need to show it again.
            if (cursorVisible || isKeyboardOverlayVisible() || isPcKeyboardVisible()) {
                inputCaptureProvider.showCursor();
            }
        }
        else {
            if (gameTouchControls != null) gameTouchControls.releaseInput();
            cancelTouchInput();
            releaseKeyboardInput();
            releaseMouseButtons();
            if (virtualController != null) {
                virtualController.releaseInput();
            }
            if (modernController != null) {
                modernController.releaseInput();
            }
            inputCaptureProvider.disableCapture();
        }

        // Grab/ungrab system keyboard shortcuts
        setMetaKeyCaptureState(grab);

        grabbedInput = grab;
        if (controllerHandler != null) {
            controllerHandler.setInputEnabled(grab && hasWindowFocus());
        }
    }

    private final Runnable toggleGrab = new Runnable() {
        @Override
        public void run() {
            setInputGrabState(!grabbedInput);
        }
    };

    private static int getModifierMask(int androidKeyCode) {
        if (androidKeyCode == KeyEvent.KEYCODE_CTRL_LEFT ||
            androidKeyCode == KeyEvent.KEYCODE_CTRL_RIGHT) {
            return KeyboardPacket.MODIFIER_CTRL << (androidKeyCode == KeyEvent.KEYCODE_CTRL_RIGHT ? 8 : 0);
        }
        else if (androidKeyCode == KeyEvent.KEYCODE_SHIFT_LEFT ||
                 androidKeyCode == KeyEvent.KEYCODE_SHIFT_RIGHT) {
            return KeyboardPacket.MODIFIER_SHIFT << (androidKeyCode == KeyEvent.KEYCODE_SHIFT_RIGHT ? 8 : 0);
        }
        else if (androidKeyCode == KeyEvent.KEYCODE_ALT_LEFT ||
                 androidKeyCode == KeyEvent.KEYCODE_ALT_RIGHT) {
            return KeyboardPacket.MODIFIER_ALT << (androidKeyCode == KeyEvent.KEYCODE_ALT_RIGHT ? 8 : 0);
        }
        else if (androidKeyCode == KeyEvent.KEYCODE_META_LEFT ||
                androidKeyCode == KeyEvent.KEYCODE_META_RIGHT) {
            return KeyboardPacket.MODIFIER_META << (androidKeyCode == KeyEvent.KEYCODE_META_RIGHT ? 8 : 0);
        }
        else {
            return 0;
        }
    }

    // Returns true if the key stroke was consumed
    private boolean handleSpecialKeys(int androidKeyCode, boolean down) {
        int modifierMask = getModifierMask(androidKeyCode);
        int nonModifierKeyCode = modifierMask == 0 ? androidKeyCode : KeyEvent.KEYCODE_UNKNOWN;

        if (down) {
            this.modifierFlags |= modifierMask;
        }
        else {
            this.modifierFlags &= ~modifierMask;
        }

        // Handle the special combos on the key up
        if (waitingForAllModifiersUp || specialKeyCode != KeyEvent.KEYCODE_UNKNOWN) {
            if (specialKeyCode == androidKeyCode) {
                // If this is a key up for the special key itself, eat that because the host never saw the original key down
                return true;
            }
            else if (modifierFlags != 0) {
                // While we're waiting for modifiers to come up, eat all key downs and allow all key ups to pass
                return down;
            }
            else {
                // When all modifiers are up, perform the special action
                switch (specialKeyCode) {
                    // Toggle input grab
                    case KeyEvent.KEYCODE_Z:
                        Handler h = getWindow().getDecorView().getHandler();
                        if (h != null) {
                            h.postDelayed(toggleGrab, 250);
                        }
                        break;

                    // Leave the stream
                    case KeyEvent.KEYCODE_Q:
                        leaveStream();
                        break;

                    case KeyEvent.KEYCODE_M:
                        showStreamMenu();
                        break;

                    case KeyEvent.KEYCODE_K:
                        togglePcKeyboard();
                        break;

                    case KeyEvent.KEYCODE_S:
                        togglePerformanceOverlay();
                        break;

                    // Toggle cursor visibility
                    case KeyEvent.KEYCODE_C:
                        toggleCursorVisibility();
                        break;

                    default:
                        break;
                }

                // Reset special key state
                specialKeyCode = KeyEvent.KEYCODE_UNKNOWN;
                waitingForAllModifiersUp = false;
            }
        }
        // Check if Ctrl+Alt+Shift is down when a non-modifier key is pressed
        else if ((getModifierState() & (KeyboardPacket.MODIFIER_CTRL | KeyboardPacket.MODIFIER_ALT | KeyboardPacket.MODIFIER_SHIFT)) ==
                (KeyboardPacket.MODIFIER_CTRL | KeyboardPacket.MODIFIER_ALT | KeyboardPacket.MODIFIER_SHIFT) &&
                (down && nonModifierKeyCode != KeyEvent.KEYCODE_UNKNOWN)) {
            switch (androidKeyCode) {
                case KeyEvent.KEYCODE_Z:
                case KeyEvent.KEYCODE_Q:
                case KeyEvent.KEYCODE_M:
                case KeyEvent.KEYCODE_K:
                case KeyEvent.KEYCODE_S:
                case KeyEvent.KEYCODE_C:
                    // Remember that a special key combo was activated, so we can consume all key
                    // events until the modifiers come up
                    specialKeyCode = androidKeyCode;
                    waitingForAllModifiersUp = true;
                    breakLoneModifierTap();
                    return true;

                default:
                    // This isn't a special combo that we consume on the client side
                    return false;
            }
        }

        // Not a special combo
        return false;
    }

    // The host has already seen Ctrl, Alt and Shift go down for a client-side combo like Ctrl+Alt+Shift+K, and
    // sees them go up. Windows reads Alt or Ctrl+Shift going up with nothing between as a tap (the menu bar,
    // the input language switch), so put a key it ignores between them.
    private void breakLoneModifierTap() {
        if (!grabbedInput || !connected || conn == null || !conn.canSendInput()) return;
        short neutral = (short) (0x8000 | PcKeyboardEngine.NEUTRAL_VK);
        byte modifiers = getModifierState();
        sendKeyboardInput(neutral, KeyboardPacket.KEY_DOWN, modifiers, (byte) 0);
        sendKeyboardInput(neutral, KeyboardPacket.KEY_UP, modifiers, (byte) 0);
    }

    // We cannot simply use modifierFlags for all key event processing, because
    // some IMEs will not generate real key events for pressing Shift. Instead
    // they will simply send key events with isShiftPressed() returning true,
    // and we will need to send the modifier flag ourselves.
    private byte getModifierState(KeyEvent event) {
        // Start with the global modifier state to ensure we cover the case
        // detailed in https://github.com/moonlight-stream/moonlight-android/issues/840
        byte modifier = getModifierState();
        if (event.isShiftPressed()) {
            modifier |= KeyboardPacket.MODIFIER_SHIFT;
        }
        if (event.isCtrlPressed()) {
            modifier |= KeyboardPacket.MODIFIER_CTRL;
        }
        if (KeyboardTranslator.altHeld(event.getMetaState(), prefConfig.rightAltAsMeta)) {
            modifier |= KeyboardPacket.MODIFIER_ALT;
        }
        if (event.isMetaPressed()) {
            modifier |= KeyboardPacket.MODIFIER_META;
        }
        return applyKeySpecificModifiers(event.getKeyCode(), modifier);
    }

    private byte getModifierState(int keyCode) {
        return applyKeySpecificModifiers(keyCode, getModifierState());
    }

    private byte applyKeySpecificModifiers(int keyCode, byte modifier) {
        if (keyCode == KeyEvent.KEYCODE_PLUS) {
            // The host protocol has a single US =/+ virtual key, so Android's semantic plus key needs Shift.
            modifier |= KeyboardPacket.MODIFIER_SHIFT;
        }
        return modifier;
    }

    private byte getModifierState() {
        return (byte) (modifierFlags | (modifierFlags >> 8));
    }

    // The keyboard options can turn a physical Back or Right Alt into the Windows key.
    private int physicalKeyCode(KeyEvent event) {
        int keyCode = event.getKeyCode();
        if (keyCode == KeyEvent.KEYCODE_DPAD_CENTER && ControllerHandler.isRemoteControlEvent(event)) {
            return KeyEvent.KEYCODE_ENTER;
        }
        boolean keyboardBack = false;
        if (keyCode == KeyEvent.KEYCODE_BACK && prefConfig.backAsMeta) {
            InputDevice device = event.getDevice();
            keyboardBack = device != null && KeyboardTranslator.isKeyboardBack(keyCode,
                    (event.getFlags() & (KeyEvent.FLAG_VIRTUAL_HARD_KEY | KeyEvent.FLAG_FALLBACK)) != 0,
                    device.isVirtual(), device.getKeyboardType() == InputDevice.KEYBOARD_TYPE_ALPHABETIC,
                    ControllerHandler.isGameControllerDevice(device), event.getSource());
        }
        return KeyboardTranslator.remapPhysicalKey(keyCode, keyboardBack, prefConfig.backAsMeta, prefConfig.rightAltAsMeta);
    }

    @Override
    public boolean onKeyDown(int keyCode, KeyEvent event) {
        return handleKeyDown(event) || super.onKeyDown(keyCode, event);
    }

    private boolean handleRemoteMenuKey(KeyEvent event) {
        if (!ControllerHandler.isRemoteControlEvent(event)) return false;
        int key = event.getKeyCode();
        if (key != KeyEvent.KEYCODE_BACK && key != KeyEvent.KEYCODE_MENU) return false;
        // These are local navigation on a TV remote, even while PC input is captured.
        if (event.getAction() == KeyEvent.ACTION_UP && !event.isCanceled()) {
            if (key == KeyEvent.KEYCODE_BACK) onBackPressed();
            else showStreamMenu();
        }
        return true;
    }

    private boolean handlePhysicalBackMenuKey(KeyEvent event) {
        if (!connected || !prefConfig.backOpensMenu || event.getKeyCode() != KeyEvent.KEYCODE_BACK) return false;
        // A handheld's dedicated Back can share its gamepad device. Reserve that physical key
        // before controller remapping, while keeping Select/Start and keyboard Windows-key mode.
        if (physicalKeyCode(event) != KeyEvent.KEYCODE_BACK) return false;
        if (event.getAction() == KeyEvent.ACTION_UP && !event.isCanceled()) showStreamMenu();
        return true;
    }

    @Override
    public boolean handleKeyDown(KeyEvent event) {
        if (handleRemoteMenuKey(event)) return true;
        // A key the keyboard took keeps being the keyboard's after it closed, so its repeats stay away from the game.
        if ((isPcKeyboardVisible() || pcKeyboardKeys.get(event.getKeyCode())) && handlePcKeyboardKey(event)) return true;
        // Pass-through virtual navigation keys
        if ((event.getFlags() & KeyEvent.FLAG_VIRTUAL_HARD_KEY) != 0) {
            return false;
        }

        // Handle a synthetic back button event that some Android OS versions
        // create as a result of a right-click. This event WILL repeat if
        // the right mouse button is held down, so we ignore those.
        int eventSource = event.getSource();
        if ((eventSource == InputDevice.SOURCE_MOUSE ||
                eventSource == InputDevice.SOURCE_MOUSE_RELATIVE) &&
                event.getKeyCode() == KeyEvent.KEYCODE_BACK) {

            // Like other mouse input, synthetic Back must respect capture. If mouse
            // navigation buttons are enabled, handleMotionEvent() handles the click.
            if (grabbedInput && !prefConfig.mouseNavButtons) {
                conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_RIGHT);
            }

            // Always return true, otherwise the back press will be propagated
            // up to the parent and finish the activity.
            return true;
        }

        if (handlePhysicalBackMenuKey(event)) return true;

        boolean handled = false;

        if (!ControllerHandler.isRemoteControlEvent(event) && ControllerHandler.isGameControllerDevice(event.getDevice())) {
            // Always try the controller handler first, unless it's an alphanumeric keyboard device.
            // Otherwise, controller handler will eat keyboard d-pad events.
            handled = controllerHandler.handleButtonDown(event);
        }

        // Try the keyboard handler if it wasn't handled as a game controller
        if (!handled) {
            int keyCode = physicalKeyCode(event);

            // Let this method take duplicate key down events
            if (handleSpecialKeys(keyCode, true)) {
                return true;
            }

            // Pass through keyboard input if we're not grabbing
            if (!grabbedInput) {
                return false;
            }
            hostKeyboard.onInput(SystemClock.uptimeMillis());

            // We'll send it as a raw key event if we have a key mapping, otherwise we'll send it
            // as UTF-8 text (if it's a printable character).
            short translated = keyboardTranslator.translate(keyCode, event.getDeviceId());
            if (translated == 0) {
                // Make sure it has a valid Unicode representation and it's not a dead character
                // (which we don't support). If those are true, we can send it as UTF-8 text.
                //
                // NB: We need to be sure this happens before the getRepeatCount() check because
                // UTF-8 events don't auto-repeat on the host side.
                int unicodeChar = event.getUnicodeChar();
                String text = KeyboardTranslator.textForCodePoint(unicodeChar);
                if (text != null) {
                    conn.sendUtf8Text(text);
                    return true;
                }

                return false;
            }

            // Eat repeat down events
            if (event.getRepeatCount() > 0) {
                return true;
            }

            sendKeyboardInput(translated, KeyboardPacket.KEY_DOWN, getModifierState(event),
                    keyboardTranslator.hasNormalizedMapping(keyCode, event.getDeviceId()) ? 0 : MoonBridge.SS_KBE_FLAG_NON_NORMALIZED);
        }

        return true;
    }

    @Override
    public boolean onKeyUp(int keyCode, KeyEvent event) {
        return handleKeyUp(event) || super.onKeyUp(keyCode, event);
    }

    @Override
    public boolean handleKeyUp(KeyEvent event) {
        if (handleRemoteMenuKey(event)) return true;
        if ((pcKeyboardKeys.size() > 0 || isPcKeyboardVisible()) && handlePcKeyboardKey(event)) return true;
        // Pass-through virtual navigation keys
        if ((event.getFlags() & KeyEvent.FLAG_VIRTUAL_HARD_KEY) != 0) {
            return false;
        }

        // Handle a synthetic back button event that some Android OS versions
        // create as a result of a right-click.
        int eventSource = event.getSource();
        if ((eventSource == InputDevice.SOURCE_MOUSE ||
                eventSource == InputDevice.SOURCE_MOUSE_RELATIVE) &&
                event.getKeyCode() == KeyEvent.KEYCODE_BACK) {

            // Releasing capture already releases held mouse buttons. Ignore any
            // late synthetic Back release, while still consuming Android navigation.
            if (grabbedInput && !prefConfig.mouseNavButtons) {
                conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_RIGHT);
            }

            // Always return true, otherwise the back press will be propagated
            // up to the parent and finish the activity.
            return true;
        }

        if (handlePhysicalBackMenuKey(event)) return true;

        boolean handled = false;
        if (!ControllerHandler.isRemoteControlEvent(event) && ControllerHandler.isGameControllerDevice(event.getDevice())) {
            // Always try the controller handler first, unless it's an alphanumeric keyboard device.
            // Otherwise, controller handler will eat keyboard d-pad events.
            handled = controllerHandler.handleButtonUp(event);
        }

        // Try the keyboard handler if it wasn't handled as a game controller
        if (!handled) {
            int keyCode = physicalKeyCode(event);
            if (handleSpecialKeys(keyCode, false)) {
                return true;
            }

            // Pass through keyboard input if we're not grabbing
            if (!grabbedInput) {
                return false;
            }

            short translated = keyboardTranslator.translate(keyCode, event.getDeviceId());
            if (translated == 0) {
                // If we sent this event as UTF-8 on key down, also report that it was handled
                // when we get the key up event for it.
                int unicodeChar = event.getUnicodeChar();
                return (unicodeChar & KeyCharacterMap.COMBINING_ACCENT) == 0 && (unicodeChar & KeyCharacterMap.COMBINING_ACCENT_MASK) != 0;
            }

            sendKeyboardInput(translated, KeyboardPacket.KEY_UP, getModifierState(event),
                    keyboardTranslator.hasNormalizedMapping(keyCode, event.getDeviceId()) ? 0 : MoonBridge.SS_KBE_FLAG_NON_NORMALIZED);
        }

        return true;
    }

    @Override
    public boolean onKeyMultiple(int keyCode, int repeatCount, KeyEvent event) {
        return handleKeyMultiple(event) || super.onKeyMultiple(keyCode, repeatCount, event);
    }

    private boolean handleKeyMultiple(KeyEvent event) {
        // We can receive keys from a software keyboard that don't correspond to any existing
        // KEYCODE value. Android will give those to us as an ACTION_MULTIPLE KeyEvent.
        //
        // Despite the fact that the Android docs say this is unused since API level 29, these
        // events are still sent as of Android 13 for the above case.
        //
        // For other cases of ACTION_MULTIPLE, we will not report those as handled so hopefully
        // they will be passed to us again as regular singular key events.
        if (event.getKeyCode() != KeyEvent.KEYCODE_UNKNOWN) {
            return false;
        }

        return handleTextInput(event.getCharacters());
    }

    @Override
    public boolean handleTextInput(CharSequence text) {
        if (!grabbedInput || conn == null || !conn.canSendInput()) {
            return false;
        }
        return KeyboardTranslator.sendTextInput(text, conn::sendUtf8Text,
                (key, direction) -> sendKeyboardInput(key, direction, (byte) 0,
                        MoonBridge.SS_KBE_FLAG_NON_NORMALIZED));
    }

    private TouchContext getTouchContext(int actionIndex)
    {
        if (actionIndex < touchContextMap.length) {
            return touchContextMap[actionIndex];
        }
        else {
            return null;
        }
    }

    @Override
    public void toggleKeyboard() {
        if (!foreground || !connected || isFinishing() || streamMenu != null || !conn.canSendInput()) {
            return;
        }
        LimeLog.info("Toggling keyboard overlay");
        boolean visible;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            WindowInsets insets = streamView.getRootWindowInsets();
            visible = insets != null && insets.isVisible(WindowInsets.Type.ime());
        } else {
            WindowInsetsCompat insets = ViewCompat.getRootWindowInsets(streamView);
            visible = insets != null && insets.isVisible(WindowInsetsCompat.Type.ime());
        }
        if (!visible) {
            hideKeyboardOverlay();
            hidePcKeyboard();
            setInputGrabState(true);
        }
        streamView.requestFocus();
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            if (visible) {
                getWindow().getInsetsController().hide(WindowInsets.Type.ime());
            } else {
                getWindow().getInsetsController().show(WindowInsets.Type.ime());
            }
        } else {
            InputMethodManager inputManager = (InputMethodManager) getSystemService(Context.INPUT_METHOD_SERVICE);
            if (visible) {
                inputManager.hideSoftInputFromWindow(streamView.getWindowToken(), 0);
            } else {
                inputManager.showSoftInput(streamView, 0);
            }
        }
    }

    private void togglePcKeyboard() {
        if (!foreground || !connected || isFinishing() || streamMenu != null || !conn.canSendInput()) {
            return;
        }
        if (isPcKeyboardVisible()) {
            hidePcKeyboard();
            return;
        }
        // One keyboard at a time: the extra-keys panel and the system keyboard make room.
        hideKeyboardOverlay();
        hideSystemKeyboard();
        if (pcKeyboard == null) {
            pcKeyboard = new PcKeyboardOverlay(this, prefConfig.pcKeyboardLayout, prefConfig.pcKeyboardSize,
                    prefConfig.pcKeyboardOpacity, this::sendPcKeyboardKey, this::hidePcKeyboard);
            ViewCompat.setOnApplyWindowInsetsListener(pcKeyboard, (view, windowInsets) -> {
                androidx.core.graphics.Insets safe = windowInsets.getInsets(
                        WindowInsetsCompat.Type.systemBars() | WindowInsetsCompat.Type.displayCutout());
                view.setPadding(safe.left, 0, safe.right, safe.bottom);
                return windowInsets;
            });
            ((FrameLayout) streamView.getParent()).addView(pcKeyboard,
                    new FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT,
                            FrameLayout.LayoutParams.WRAP_CONTENT, android.view.Gravity.BOTTOM));
            ViewCompat.requestApplyInsets(pcKeyboard);
        } else {
            pcKeyboard.setVisibility(View.VISIBLE);
            pcKeyboard.bringToFront();
        }
        // The pads steer the keyboard until it closes. The capture is set before input is enabled, so no
        // report can reach the game in between; setPadCapture lets go of whatever the game held, and each
        // USB pad does the same from its own thread on its first report.
        controllerHandler.setPadCapture(new ControllerHandler.PadCapture() {
            @Override public void button(int flag, boolean down) {
                PcKeyboardController.Pad pad = padForFlag(flag);
                if (pad != null) runOnUiThread(() -> padToPcKeyboard(pad, down));
            }
            @Override public void stick(int controllerId, float x, float y) {
                // USB-driver pads have no Android device id; a negative one cannot clash with a real device.
                runOnUiThread(() -> {
                    if (isPcKeyboardVisible()) steerPcKeyboard(-2 - controllerId, x, y, SystemClock.uptimeMillis());
                });
            }
        });
        // A release that leaves the keyboard open (a layout switch) must not leave a held USB stick stale.
        pcKeyboard.controller().setOnReleased(controllerHandler::resyncPadCapture);
        setInputGrabState(true);
        streamView.requestFocus();
    }

    private boolean isPcKeyboardVisible() {
        return pcKeyboard != null && pcKeyboard.getVisibility() == View.VISIBLE;
    }

    /** Closing lets go of every key and modifier it pressed, and gives the pads back to the game. */
    private void hidePcKeyboard() {
        if (pcKeyboard == null || pcKeyboard.getVisibility() != View.VISIBLE) return;
        pcKeyboard.releaseInputs();
        pcKeyboardAxisDevice = -1;
        pcKeyboard.setVisibility(View.GONE);
        if (controllerHandler != null) controllerHandler.setPadCapture(null);
        if (grabbedInput && !cursorVisible && !isKeyboardOverlayVisible() && inputCaptureProvider != null) {
            inputCaptureProvider.hideCursor();
        }
    }

    private void sendPcKeyboardKey(int vk, boolean down, byte modifiers) {
        if (!connected || conn == null || !conn.canSendInput()) return;
        sendKeyboardInput((short) (0x8000 | vk), down ? KeyboardPacket.KEY_DOWN : KeyboardPacket.KEY_UP, modifiers, (byte) 0);
    }

    private void hideSystemKeyboard() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            WindowInsets insets = streamView.getRootWindowInsets();
            if (insets != null && insets.isVisible(WindowInsets.Type.ime())) {
                getWindow().getInsetsController().hide(WindowInsets.Type.ime());
            }
        } else {
            InputMethodManager inputManager = (InputMethodManager) getSystemService(Context.INPUT_METHOD_SERVICE);
            inputManager.hideSoftInputFromWindow(streamView.getWindowToken(), 0);
        }
    }

    private void padToPcKeyboard(PcKeyboardController.Pad pad, boolean down) {
        if (isPcKeyboardVisible()) pcKeyboard.controller().pad(pad, down, SystemClock.uptimeMillis());
    }

    private static PcKeyboardController.Pad padForFlag(int flag) {
        switch (flag) {
            case ControllerPacket.UP_FLAG: return PcKeyboardController.Pad.UP;
            case ControllerPacket.DOWN_FLAG: return PcKeyboardController.Pad.DOWN;
            case ControllerPacket.LEFT_FLAG: return PcKeyboardController.Pad.LEFT;
            case ControllerPacket.RIGHT_FLAG: return PcKeyboardController.Pad.RIGHT;
            case ControllerPacket.A_FLAG: return PcKeyboardController.Pad.A;
            case ControllerPacket.B_FLAG: return PcKeyboardController.Pad.B;
            case ControllerPacket.X_FLAG: return PcKeyboardController.Pad.X;
            case ControllerPacket.Y_FLAG: return PcKeyboardController.Pad.Y;
            case ControllerPacket.LB_FLAG: return PcKeyboardController.Pad.LB;
            case ControllerPacket.RB_FLAG: return PcKeyboardController.Pad.RB;
            case ControllerPacket.PLAY_FLAG: return PcKeyboardController.Pad.START;
            default: return null;
        }
    }

    private PcKeyboardController.Pad padForKey(int keyCode, boolean flip) {
        switch (keyCode) {
            case KeyEvent.KEYCODE_DPAD_UP: return PcKeyboardController.Pad.UP;
            case KeyEvent.KEYCODE_DPAD_DOWN: return PcKeyboardController.Pad.DOWN;
            case KeyEvent.KEYCODE_DPAD_LEFT: return PcKeyboardController.Pad.LEFT;
            case KeyEvent.KEYCODE_DPAD_RIGHT: return PcKeyboardController.Pad.RIGHT;
            // OK on a TV remote.
            case KeyEvent.KEYCODE_DPAD_CENTER:
            case KeyEvent.KEYCODE_ENTER:
            case KeyEvent.KEYCODE_NUMPAD_ENTER: return PcKeyboardController.Pad.A;
            case KeyEvent.KEYCODE_BUTTON_A: return flip ? PcKeyboardController.Pad.B : PcKeyboardController.Pad.A;
            case KeyEvent.KEYCODE_BUTTON_B: return flip ? PcKeyboardController.Pad.A : PcKeyboardController.Pad.B;
            case KeyEvent.KEYCODE_BUTTON_X: return flip ? PcKeyboardController.Pad.Y : PcKeyboardController.Pad.X;
            case KeyEvent.KEYCODE_BUTTON_Y: return flip ? PcKeyboardController.Pad.X : PcKeyboardController.Pad.Y;
            case KeyEvent.KEYCODE_BUTTON_L1: return PcKeyboardController.Pad.LB;
            case KeyEvent.KEYCODE_BUTTON_R1: return PcKeyboardController.Pad.RB;
            // Menu is Start, as in the game's own button mapping.
            case KeyEvent.KEYCODE_MENU:
            case KeyEvent.KEYCODE_BUTTON_START: return PcKeyboardController.Pad.START;
            default: return null;
        }
    }

    /**
     * While the PC keyboard is open a controller or TV remote steers it, not the game. Everything on a
     * controller is consumed, so no button reaches the game; a physical keyboard still types to the PC.
     */
    private boolean handlePcKeyboardKey(KeyEvent event) {
        boolean remote = ControllerHandler.isRemoteControlEvent(event);
        if (!remote && !isRealGamepad(event.getDevice())) return false;
        int keyCode = event.getKeyCode();
        boolean down = event.getAction() == KeyEvent.ACTION_DOWN;
        if (!isPcKeyboardVisible()) {
            // A release of a key the keyboard took stays with it, even if it just closed. A new press
            // is the game's, so an old entry for the same key must not swallow its release.
            if (down) {
                // Held past the key repeat after the keyboard closed (B closes it): the host never saw the
                // press, so it must not see the repeats, and the release stays with the keyboard.
                if (event.getRepeatCount() > 0 && pcKeyboardKeys.get(keyCode)) return true;
                pcKeyboardKeys.delete(keyCode);
                return false;
            }
            boolean taken = pcKeyboardKeys.get(keyCode);
            pcKeyboardKeys.delete(keyCode);
            return taken;
        }
        // The user's button map for this pad replaces face-button flipping, as it does for the game.
        int mapped = remote || controllerHandler == null ? ControllerButtonMap.UNMAPPED : controllerHandler.userMappedKey(event);
        PcKeyboardController.Pad pad = mapped == ControllerButtonMap.UNMAPPED
                ? padForKey(keyCode, prefConfig.flipFaceButtons) : padForKey(mapped, false);
        // A pad's own Back (a handheld's Back key, or a pad that sends Back for B) closes the keyboard like B:
        // Android's TV rules name both as Back, and swallowing it would leave the pad with no way out.
        if (pad == null && !remote && (mapped == ControllerButtonMap.UNMAPPED ? keyCode : mapped) == KeyEvent.KEYCODE_BACK) {
            pad = PcKeyboardController.Pad.B;
        }
        if (pad == null) return !remote;
        if (down) {
            pcKeyboardKeys.put(keyCode, true);
            // The keyboard repeats keys itself.
            if (event.getRepeatCount() > 0) return true;
        } else {
            pcKeyboardKeys.delete(keyCode);
        }
        pcKeyboard.controller().pad(pad, down, event.getEventTime());
        return true;
    }

    /** A controller that really exists with gamepad or joystick sources; the synthetic or missing device does not count. */
    private static boolean isRealGamepad(InputDevice device) {
        if (device == null || device.getId() == -1) return false;
        int sources = device.getSources();
        return (sources & InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD
                || (sources & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK;
    }

    /** A controller went away: its held buttons would never come up, so the keyboard lets go of them. */
    private void releasePcKeyboardPads() {
        pcKeyboardKeys.clear();
        pcKeyboardAxisDevice = -1;
        if (pcKeyboard != null) pcKeyboard.controller().releasePads(SystemClock.uptimeMillis());
    }

    /**
     * The hat and the left stick steer the keyboard like the D-pad keys; both name the same direction,
     * so a pad that reports its D-pad as keys and as a hat moves once. Only one pad steers at a time:
     * a second pad resting at centre must not cancel a direction held on the first.
     */
    private void handlePcKeyboardMotion(MotionEvent event) {
        int action = event.getActionMasked();
        int device = event.getDeviceId();
        if (action == MotionEvent.ACTION_CANCEL) {
            if (device == pcKeyboardAxisDevice) pcKeyboard.controller().stick(0, 0, event.getEventTime());
            pcKeyboardAxisDevice = -1;
            return;
        }
        if (action != MotionEvent.ACTION_MOVE) return;
        float hatX = event.getAxisValue(MotionEvent.AXIS_HAT_X), hatY = event.getAxisValue(MotionEvent.AXIS_HAT_Y);
        float x = hatX != 0 ? hatX : event.getAxisValue(MotionEvent.AXIS_X);
        float y = hatY != 0 ? hatY : event.getAxisValue(MotionEvent.AXIS_Y);
        steerPcKeyboard(device, x, y, event.getEventTime());
    }

    /** The one place a stick or hat reaches the keyboard, for Android pads and USB-driver pads alike. */
    private void steerPcKeyboard(int source, float x, float y, long time) {
        PcKeyboardController keyboard = pcKeyboard.controller();
        if (pcKeyboardAxisDevice != -1 && pcKeyboardAxisDevice != source
                && PcKeyboardController.stickDirection(x, y, null) == null) {
            return;
        }
        keyboard.stick(x, y, time);
        pcKeyboardAxisDevice = keyboard.stickActive() ? source : -1;
    }

    private void toggleKeyboardOverlay() {
        if (!foreground || !connected || isFinishing() || streamMenu != null || !conn.canSendInput()) {
            return;
        }
        if (isKeyboardOverlayVisible()) {
            hideKeyboardOverlay();
            return;
        }
        hidePcKeyboard();
        if (keyboardOverlay == null) {
            keyboardOverlay = new KeyboardOverlay(this, new DesktopControlsPanel.Sink() {
                @Override public void sendKeys(int[] keys) {
                    if (canSendDesktopInput()) sendKeyboardShortcut(keys);
                }
                @Override public void move(float dx, float dy) { sendDesktopMotion(dx, dy); }
                @Override public void scroll(float vertical, float horizontal) { sendDesktopScroll(vertical, horizontal); }
                @Override public void mouseButton(int button, boolean down) { sendDesktopButton(button, down); }
            }, () -> {
                hideKeyboardOverlay();
                toggleKeyboard();
            }, this::hideKeyboardOverlay);
            ViewCompat.setOnApplyWindowInsetsListener(keyboardOverlay, (view, windowInsets) -> {
                androidx.core.graphics.Insets safe = windowInsets.getInsets(
                        WindowInsetsCompat.Type.systemBars() | WindowInsetsCompat.Type.displayCutout());
                float density = getResources().getDisplayMetrics().density;
                int horizontal = Math.round(4 * density), vertical = Math.round(3 * density);
                view.setPadding(safe.left + horizontal, vertical, safe.right + horizontal, safe.bottom + vertical);
                return windowInsets;
            });
            ((FrameLayout) streamView.getParent()).addView(keyboardOverlay,
                    new FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT,
                            FrameLayout.LayoutParams.WRAP_CONTENT, android.view.Gravity.BOTTOM));
            ViewCompat.requestApplyInsets(keyboardOverlay);
        } else {
            keyboardOverlay.reloadConfiguration();
            keyboardOverlay.setVisibility(View.VISIBLE);
            keyboardOverlay.bringToFront();
        }
        setInputGrabState(true);
        streamView.requestFocus();
    }

    private boolean isKeyboardOverlayVisible() {
        return keyboardOverlay != null && keyboardOverlay.getVisibility() == View.VISIBLE;
    }

    private void hideKeyboardOverlay() {
        if (keyboardOverlay == null) return;
        releaseDesktopInput();
        keyboardOverlay.setVisibility(View.GONE);
        if (grabbedInput && !cursorVisible && inputCaptureProvider != null) {
            inputCaptureProvider.hideCursor();
        }
    }

    private boolean canSendDesktopInput() {
        return foreground && connected && grabbedInput && (desktopWindowFocused || dispatchingDesktopTouch) && !isFinishing() &&
                streamMenu == null && conn != null && conn.canSendInput() && isKeyboardOverlayVisible();
    }

    private void sendDesktopMotion(float dx, float dy) {
        if (!canSendDesktopInput() || !Float.isFinite(dx) || !Float.isFinite(dy)) return;
        if (trackpadPointer != null) {
            if (streamView.getWidth() <= 0 || streamView.getHeight() <= 0) return;
            float zoom = trackpadPointer.isLocalCursor() ? trackpadPointer.zoomScale() : 1;
            dx *= prefConfig.width / (streamView.getWidth() * zoom);
            dy *= prefConfig.height / (streamView.getHeight() * zoom);
            trackpadPointer.stopMomentum();
        }
        short x = desktopDelta(desktopDeltaX, dx);
        short y = desktopDelta(desktopDeltaY, dy);
        if (x != 0 || y != 0) {
            if (trackpadPointer != null) trackpadPointer.moveBy(x, y);
            else conn.sendMouseMove(x, y);
        }
    }

    private void sendDesktopScroll(float vertical, float horizontal) {
        if (!canSendDesktopInput() || !Float.isFinite(vertical) || !Float.isFinite(horizontal)) return;
        short v = desktopDelta(desktopScrollV, vertical);
        short h = desktopDelta(desktopScrollH, horizontal);
        if (v != 0) conn.sendMouseHighResScroll(v);
        if (h != 0) conn.sendMouseHighResHScroll(h);
    }

    private static short desktopDelta(MouseDeltaAccumulator accumulator, float value) {
        int delta = accumulator.scale(Math.max(Short.MIN_VALUE, Math.min(Short.MAX_VALUE, value)), 1);
        return (short) Math.max(Short.MIN_VALUE, Math.min(Short.MAX_VALUE, delta));
    }

    private void sendDesktopButton(int button, boolean down) {
        if (button < 1 || button > 5 || conn == null || (down && !canSendDesktopInput())) return;
        int mask = 1 << button;
        boolean wasDown = (desktopHeldButtons & mask) != 0;
        if (wasDown == down) return;
        if (down) {
            desktopHeldButtons |= mask;
            if (trackpadPointer != null) trackpadPointer.stopMomentum();
        }
        else desktopHeldButtons &= ~mask;
        // Releases must also run after focus/grab was lost. The connection merges this panel's
        // ownership separately from physical input and per-game touch controls.
        conn.sendDesktopMouseButton((byte) button, down);
    }

    private void releaseDesktopInput() {
        if (keyboardOverlay != null) keyboardOverlay.releaseInputs();
        // Also clear ownership if a view was removed or interrupted between its callbacks.
        for (int button = 1; button <= 5; button++) sendDesktopButton(button, false);
        desktopDeltaX = new MouseDeltaAccumulator();
        desktopDeltaY = new MouseDeltaAccumulator();
        desktopScrollV = new MouseDeltaAccumulator();
        desktopScrollH = new MouseDeltaAccumulator();
    }

    private void sendKeyboardShortcut(int... keys) {
        if (!foreground || !connected || !grabbedInput || streamMenu != null || !conn.canSendInput()) {
            return;
        }
        byte modifiers = getModifierState();
        boolean[] sent = new boolean[keys.length];
        for (int i = 0; i < keys.length; i++) {
            short key = keyboardTranslator.translate(keys[i], -1);
            int mask = getModifierMask(keys[i]);
            modifiers |= (byte) (mask | (mask >> 8));
            if (!pressedKeys.containsKey(key)) {
                sendKeyboardInput(key, KeyboardPacket.KEY_DOWN, modifiers, (byte) 0);
                sent[i] = true;
            }
        }
        for (int i = keys.length - 1; i >= 0; i--) {
            if (sent[i]) {
                int mask = getModifierMask(keys[i]);
                modifiers &= ~(mask | (mask >> 8));
                modifiers |= getModifierState();
                sendKeyboardInput(keyboardTranslator.translate(keys[i], -1), KeyboardPacket.KEY_UP, modifiers, (byte) 0);
            }
        }
    }

    private byte getLiTouchTypeFromEvent(MotionEvent event) {
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
            case MotionEvent.ACTION_POINTER_DOWN:
                return MoonBridge.LI_TOUCH_EVENT_DOWN;

            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_POINTER_UP:
                if ((event.getFlags() & MotionEvent.FLAG_CANCELED) != 0) {
                    return MoonBridge.LI_TOUCH_EVENT_CANCEL;
                }
                else {
                    return MoonBridge.LI_TOUCH_EVENT_UP;
                }

            case MotionEvent.ACTION_MOVE:
                return MoonBridge.LI_TOUCH_EVENT_MOVE;

            case MotionEvent.ACTION_CANCEL:
                // ACTION_CANCEL applies to *all* pointers in the gesture, so it maps to CANCEL_ALL
                // rather than CANCEL. For a single pointer cancellation, that's indicated via
                // FLAG_CANCELED on a ACTION_POINTER_UP.
                // https://developer.android.com/develop/ui/views/touch-and-input/gestures/multi
                return MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL;

            case MotionEvent.ACTION_HOVER_ENTER:
            case MotionEvent.ACTION_HOVER_MOVE:
                return MoonBridge.LI_TOUCH_EVENT_HOVER;

            case MotionEvent.ACTION_HOVER_EXIT:
                return MoonBridge.LI_TOUCH_EVENT_HOVER_LEAVE;

            case MotionEvent.ACTION_BUTTON_PRESS:
            case MotionEvent.ACTION_BUTTON_RELEASE:
                return MoonBridge.LI_TOUCH_EVENT_BUTTON_ONLY;

            default:
               return -1;
        }
    }

    private float[] getStreamViewRelativeNormalizedXY(View view, MotionEvent event, int pointerIndex) {
        float normalizedX = event.getX(pointerIndex) + getStreamViewOffsetX(view);
        float normalizedY = event.getY(pointerIndex) + getStreamViewOffsetY(view);

        normalizedX = Math.max(normalizedX, 0.0f);
        normalizedY = Math.max(normalizedY, 0.0f);

        normalizedX = Math.min(normalizedX, streamView.getWidth());
        normalizedY = Math.min(normalizedY, streamView.getHeight());

        normalizedX /= streamView.getWidth();
        normalizedY /= streamView.getHeight();

        return new float[] { normalizedX, normalizedY };
    }

    private float getStreamViewOffsetX(View view) {
        // Sibling views share the pane padding, including fold and display-cutout insets.
        return (view != null ? view.getX() : 0) - streamView.getX();
    }

    private float getStreamViewOffsetY(View view) {
        return (view != null ? view.getY() : 0) - streamView.getY();
    }

    private static float normalizeValueInRange(float value, InputDevice.MotionRange range) {
        return (value - range.getMin()) / range.getRange();
    }

    private static float getPressureOrDistance(MotionEvent event, int pointerIndex) {
        InputDevice dev = event.getDevice();
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_HOVER_ENTER:
            case MotionEvent.ACTION_HOVER_MOVE:
            case MotionEvent.ACTION_HOVER_EXIT:
                // Hover events report distance
                if (dev != null) {
                    InputDevice.MotionRange distanceRange = dev.getMotionRange(MotionEvent.AXIS_DISTANCE, event.getSource());
                    if (distanceRange != null) {
                        return normalizeValueInRange(event.getAxisValue(MotionEvent.AXIS_DISTANCE, pointerIndex), distanceRange);
                    }
                }
                return 0.0f;

            default:
                // Other events report pressure
                return event.getPressure(pointerIndex);
        }
    }

    private static short getRotationDegrees(MotionEvent event, int pointerIndex) {
        InputDevice dev = event.getDevice();
        if (dev != null) {
            if (dev.getMotionRange(MotionEvent.AXIS_ORIENTATION, event.getSource()) != null) {
                short rotationDegrees = (short) Math.toDegrees(event.getOrientation(pointerIndex));
                if (rotationDegrees < 0) {
                    rotationDegrees += 360;
                }
                return rotationDegrees;
            }
        }
        return MoonBridge.LI_ROT_UNKNOWN;
    }

    private static float[] polarToCartesian(float r, float theta) {
        return new float[] { (float)(r * Math.cos(theta)), (float)(r * Math.sin(theta)) };
    }

    private static float cartesianToR(float[] point) {
        return (float)Math.sqrt(Math.pow(point[0], 2) + Math.pow(point[1], 2));
    }

    private float[] getStreamViewNormalizedContactArea(MotionEvent event, int pointerIndex) {
        float orientation;

        // If the orientation is unknown, we'll just assume it's at a 45 degree angle and scale it by
        // X and Y scaling factors evenly.
        if (event.getDevice() == null || event.getDevice().getMotionRange(MotionEvent.AXIS_ORIENTATION, event.getSource()) == null) {
            orientation = (float)(Math.PI / 4);
        }
        else {
            orientation = event.getOrientation(pointerIndex);
        }

        float contactAreaMajor, contactAreaMinor;
        switch (event.getActionMasked()) {
            // Hover events report the tool size
            case MotionEvent.ACTION_HOVER_ENTER:
            case MotionEvent.ACTION_HOVER_MOVE:
            case MotionEvent.ACTION_HOVER_EXIT:
                contactAreaMajor = event.getToolMajor(pointerIndex);
                contactAreaMinor = event.getToolMinor(pointerIndex);
                break;

            // Other events report contact area
            default:
                contactAreaMajor = event.getTouchMajor(pointerIndex);
                contactAreaMinor = event.getTouchMinor(pointerIndex);
                break;
        }

        // The contact area major axis is parallel to the orientation, so we simply convert
        // polar to cartesian coordinates using the orientation as theta.
        float[] contactAreaMajorCartesian = polarToCartesian(contactAreaMajor, orientation);

        // The contact area minor axis is perpendicular to the contact area major axis (and thus
        // the orientation), so rotate the orientation angle by 90 degrees.
        float[] contactAreaMinorCartesian = polarToCartesian(contactAreaMinor, (float)(orientation + (Math.PI / 2)));

        // Normalize the contact area to the stream view size
        contactAreaMajorCartesian[0] = Math.min(Math.abs(contactAreaMajorCartesian[0]), streamView.getWidth()) / streamView.getWidth();
        contactAreaMinorCartesian[0] = Math.min(Math.abs(contactAreaMinorCartesian[0]), streamView.getWidth()) / streamView.getWidth();
        contactAreaMajorCartesian[1] = Math.min(Math.abs(contactAreaMajorCartesian[1]), streamView.getHeight()) / streamView.getHeight();
        contactAreaMinorCartesian[1] = Math.min(Math.abs(contactAreaMinorCartesian[1]), streamView.getHeight()) / streamView.getHeight();

        // Convert the normalized values back into polar coordinates
        return new float[] { cartesianToR(contactAreaMajorCartesian), cartesianToR(contactAreaMinorCartesian) };
    }

    private boolean sendPenEventForPointer(View view, MotionEvent event, byte eventType, byte toolType, int pointerIndex) {
        byte penButtons = 0;
        if ((event.getButtonState() & MotionEvent.BUTTON_STYLUS_PRIMARY) != 0) {
            penButtons |= MoonBridge.LI_PEN_BUTTON_PRIMARY;
        }
        if ((event.getButtonState() & MotionEvent.BUTTON_STYLUS_SECONDARY) != 0) {
            penButtons |= MoonBridge.LI_PEN_BUTTON_SECONDARY;
        }

        byte tiltDegrees = MoonBridge.LI_TILT_UNKNOWN;
        InputDevice dev = event.getDevice();
        if (dev != null) {
            if (dev.getMotionRange(MotionEvent.AXIS_TILT, event.getSource()) != null) {
                tiltDegrees = (byte)Math.toDegrees(event.getAxisValue(MotionEvent.AXIS_TILT, pointerIndex));
            }
        }

        float[] normalizedCoords = getStreamViewRelativeNormalizedXY(view, event, pointerIndex);
        float[] normalizedContactArea = getStreamViewNormalizedContactArea(event, pointerIndex);
        return conn.sendPenEvent(eventType, toolType, penButtons,
                normalizedCoords[0], normalizedCoords[1],
                getPressureOrDistance(event, pointerIndex),
                normalizedContactArea[0], normalizedContactArea[1],
                getRotationDegrees(event, pointerIndex), tiltDegrees) != MoonBridge.LI_ERR_UNSUPPORTED;
    }

    private static byte convertToolTypeToStylusToolType(MotionEvent event, int pointerIndex) {
        switch (event.getToolType(pointerIndex)) {
            case MotionEvent.TOOL_TYPE_ERASER:
                return MoonBridge.LI_TOOL_TYPE_ERASER;
            case MotionEvent.TOOL_TYPE_STYLUS:
                return MoonBridge.LI_TOOL_TYPE_PEN;
            default:
                return MoonBridge.LI_TOOL_TYPE_UNKNOWN;
        }
    }

    private boolean trySendPenEvent(View view, MotionEvent event) {
        byte eventType = getLiTouchTypeFromEvent(event);
        if (eventType < 0) {
            return false;
        }

        if (event.getActionMasked() == MotionEvent.ACTION_MOVE) {
            // Move events may impact all active pointers
            boolean handledStylusEvent = false;
            for (int i = 0; i < event.getPointerCount(); i++) {
                byte toolType = convertToolTypeToStylusToolType(event, i);
                if (toolType == MoonBridge.LI_TOOL_TYPE_UNKNOWN) {
                    // Not a stylus pointer, so skip it
                    continue;
                }
                else {
                    // This pointer is a stylus, so we'll report that we handled this event
                    handledStylusEvent = true;
                }

                if (!sendPenEventForPointer(view, event, eventType, toolType, i)) {
                    // Pen events aren't supported by the host
                    return false;
                }
            }
            return handledStylusEvent;
        }
        else if (event.getActionMasked() == MotionEvent.ACTION_CANCEL) {
            // Cancel impacts all active pointers
            return conn.sendPenEvent(MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL, MoonBridge.LI_TOOL_TYPE_UNKNOWN, (byte)0,
                    0, 0, 0, 0, 0,
                    MoonBridge.LI_ROT_UNKNOWN, MoonBridge.LI_TILT_UNKNOWN) != MoonBridge.LI_ERR_UNSUPPORTED;
        }
        else {
            // Up, Down, and Hover events are specific to the action index
            byte toolType = convertToolTypeToStylusToolType(event, event.getActionIndex());
            if (toolType == MoonBridge.LI_TOOL_TYPE_UNKNOWN) {
                // Not a stylus event
                return false;
            }
            return sendPenEventForPointer(view, event, eventType, toolType, event.getActionIndex());
        }
    }

    private boolean sendTouchEventForPointer(View view, MotionEvent event, byte eventType, int pointerIndex) {
        float[] normalizedCoords = getStreamViewRelativeNormalizedXY(view, event, pointerIndex);
        float[] normalizedContactArea = getStreamViewNormalizedContactArea(event, pointerIndex);
        return conn.sendTouchEvent(eventType, event.getPointerId(pointerIndex),
                normalizedCoords[0], normalizedCoords[1],
                getPressureOrDistance(event, pointerIndex),
                normalizedContactArea[0], normalizedContactArea[1],
                getRotationDegrees(event, pointerIndex)) != MoonBridge.LI_ERR_UNSUPPORTED;
    }

    private boolean trySendTouchEvent(View view, MotionEvent event) {
        byte eventType = getLiTouchTypeFromEvent(event);
        if (eventType < 0) {
            return false;
        }

        if (event.getActionMasked() == MotionEvent.ACTION_MOVE) {
            // Move events may impact all active pointers
            for (int i = 0; i < event.getPointerCount(); i++) {
                if (!sendTouchEventForPointer(view, event, eventType, i)) {
                    return false;
                }
            }
            return true;
        }
        else if (event.getActionMasked() == MotionEvent.ACTION_CANCEL) {
            // Cancel impacts all active pointers
            return conn.sendTouchEvent(MoonBridge.LI_TOUCH_EVENT_CANCEL_ALL, 0,
                    0, 0, 0, 0, 0,
                    MoonBridge.LI_ROT_UNKNOWN) != MoonBridge.LI_ERR_UNSUPPORTED;
        }
        else {
            // Up, Down, and Hover events are specific to the action index
            return sendTouchEventForPointer(view, event, eventType, event.getActionIndex());
        }
    }

    // Returns true if the event was consumed
    // NB: View is only present if called from a view callback
    private boolean handleMotionEvent(View view, MotionEvent event) {
        noteStreamPress(view, event);
        if (gameTouchControls != null && gameTouchControls.isEditing() &&
                (event.getSource() & InputDevice.SOURCE_CLASS_POINTER) != 0) return true;
        if (desktopGestures != null && desktopGestures.enabled() &&
                desktopGestures.viewport().scale() != 1 && event.getPointerCount() > 0 &&
                (event.getSource() & InputDevice.SOURCE_CLASS_POINTER) != 0 &&
                (!prefConfig.touchscreenTrackpad || event.getToolType(0) != MotionEvent.TOOL_TYPE_FINGER)) {
            MotionEvent translated = MotionEvent.obtain(event);
            translated.offsetLocation(getStreamViewOffsetX(view), getStreamViewOffsetY(view));
            com.limelight.ui.DesktopViewport viewport = desktopGestures.viewport();
            android.graphics.Matrix inverse = new android.graphics.Matrix();
            inverse.setValues(new float[] {1 / viewport.scale(), 0, -viewport.x() / viewport.scale(),
                    0, 1 / viewport.scale(), -viewport.y() / viewport.scale(), 0, 0, 1});
            translated.transform(inverse);
            try { return handleMotionEventUntransformed(streamView, translated); }
            finally { translated.recycle(); }
        }
        return handleMotionEventUntransformed(view, event);
    }

    private boolean handleMotionEventUntransformed(View view, MotionEvent event) {
        // Pass through mouse/touch/joystick input if we're not grabbing
        if (!grabbedInput) {
            return false;
        }

        int eventSource = event.getSource();
        int deviceSources = event.getDevice() != null ? event.getDevice().getSources() : 0;
        if ((eventSource & InputDevice.SOURCE_CLASS_JOYSTICK) != 0) {
            if (isPcKeyboardVisible()) {
                handlePcKeyboardMotion(event);
                return true;
            }
            if (controllerHandler.handleMotionEvent(event)) {
                return true;
            }
        }
        else if ((deviceSources & InputDevice.SOURCE_CLASS_JOYSTICK) != 0 && controllerHandler.tryHandleTouchpadEvent(event)) {
            return true;
        }
        else if ((eventSource & InputDevice.SOURCE_CLASS_POINTER) != 0 ||
                 (eventSource & InputDevice.SOURCE_CLASS_POSITION) != 0 ||
                 eventSource == InputDevice.SOURCE_MOUSE_RELATIVE)
        {
            // This case is for mice and non-finger touch devices
            if (eventSource == InputDevice.SOURCE_MOUSE ||
                    (eventSource & InputDevice.SOURCE_CLASS_POSITION) != 0 || // SOURCE_TOUCHPAD
                    eventSource == InputDevice.SOURCE_MOUSE_RELATIVE ||
                    (event.getPointerCount() >= 1 &&
                            (event.getToolType(0) == MotionEvent.TOOL_TYPE_MOUSE ||
                                    event.getToolType(0) == MotionEvent.TOOL_TYPE_STYLUS ||
                                    event.getToolType(0) == MotionEvent.TOOL_TYPE_ERASER)) ||
                    eventSource == 12290) // 12290 = Samsung DeX mode desktop mouse
            {
                int buttonState = event.getButtonState();
                int changedButtons = buttonState ^ lastButtonState;

                // The DeX touchpad on the Fold 4 sends proper right click events using BUTTON_SECONDARY,
                // but doesn't send BUTTON_PRIMARY for a regular click. Instead it sends ACTION_DOWN/UP,
                // so we need to fix that up to look like a sane input event to process it correctly.
                if (eventSource == 12290) {
                    if (event.getActionMasked() == MotionEvent.ACTION_DOWN) {
                        buttonState |= MotionEvent.BUTTON_PRIMARY;
                    }
                    else if (event.getAction() == MotionEvent.ACTION_UP) {
                        buttonState &= ~MotionEvent.BUTTON_PRIMARY;
                    }
                    else {
                        // We may be faking the primary button down from a previous event,
                        // so be sure to add that bit back into the button state.
                        buttonState |= (lastButtonState & MotionEvent.BUTTON_PRIMARY);
                    }

                    changedButtons = buttonState ^ lastButtonState;
                }

                // Ignore mouse input if we're not capturing from our input source
                if (!inputCaptureProvider.isCapturingActive()) {
                    // We return true here because otherwise the events may end up causing
                    // Android to synthesize d-pad events.
                    return true;
                }

                // Always update the position before sending any button events. If we're
                // dealing with a stylus without hover support, our position might be
                // significantly different than before.
                if (inputCaptureProvider.eventHasRelativeMouseAxes(event)) {
                    short deltaX = (short)mouseDeltaX.scale(inputCaptureProvider.getRelativeAxisX(event), prefConfig.mouseSpeed / 100f);
                    short deltaY = (short)mouseDeltaY.scale(inputCaptureProvider.getRelativeAxisY(event), prefConfig.mouseSpeed / 100f);

                    if (deltaX != 0 || deltaY != 0) {
                        if (prefConfig.absoluteMouseMode) {
                            // NB: view may be null, but we can unconditionally use streamView because we don't need to adjust
                            // relative axis deltas for the position of the streamView within the parent's coordinate system.
                            conn.sendMouseMoveAsMousePosition(deltaX, deltaY, (short)streamView.getWidth(), (short)streamView.getHeight());
                        }
                        else {
                            conn.sendMouseMove(deltaX, deltaY);
                        }
                    }
                }
                else if ((eventSource & InputDevice.SOURCE_CLASS_POSITION) != 0) {
                    // If this input device is not associated with the view itself (like a trackpad),
                    // we'll convert the device-specific coordinates to use to send the cursor position.
                    // This really isn't ideal but it's probably better than nothing.
                    //
                    // Trackpad on newer versions of Android (Oreo and later) should be caught by the
                    // relative axes case above. If we get here, we're on an older version that doesn't
                    // support pointer capture.
                    InputDevice device = event.getDevice();
                    if (device != null) {
                        InputDevice.MotionRange xRange = device.getMotionRange(MotionEvent.AXIS_X, eventSource);
                        InputDevice.MotionRange yRange = device.getMotionRange(MotionEvent.AXIS_Y, eventSource);

                        // All touchpads coordinate planes should start at (0, 0)
                        if (xRange != null && yRange != null && xRange.getMin() == 0 && yRange.getMin() == 0) {
                            int xMax = (int)xRange.getMax();
                            int yMax = (int)yRange.getMax();

                            // Touchpads must be smaller than (65535, 65535)
                            if (xMax <= Short.MAX_VALUE && yMax <= Short.MAX_VALUE) {
                                conn.sendMousePosition((short)event.getX(), (short)event.getY(),
                                                       (short)xMax, (short)yMax);
                            }
                        }
                    }
                }
                else if (view != null && trySendPenEvent(view, event)) {
                    // If our host supports pen events, send it directly
                    return true;
                }
                else if (view != null) {
                    // Otherwise send absolute position based on the view for SOURCE_CLASS_POINTER
                    updateMousePosition(view, event);
                }

                if (event.getActionMasked() == MotionEvent.ACTION_SCROLL) {
                    // Send the vertical scroll packet
                    conn.sendMouseHighResScroll((short)(event.getAxisValue(MotionEvent.AXIS_VSCROLL) * 120));
                    conn.sendMouseHighResHScroll((short)(event.getAxisValue(MotionEvent.AXIS_HSCROLL) * 120));
                }

                if ((changedButtons & MotionEvent.BUTTON_PRIMARY) != 0) {
                    if ((buttonState & MotionEvent.BUTTON_PRIMARY) != 0) {
                        conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_LEFT);
                    }
                    else {
                        conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_LEFT);
                    }
                }

                // Mouse secondary or stylus primary is right click (stylus down is left click)
                if ((changedButtons & (MotionEvent.BUTTON_SECONDARY | MotionEvent.BUTTON_STYLUS_PRIMARY)) != 0) {
                    if ((buttonState & (MotionEvent.BUTTON_SECONDARY | MotionEvent.BUTTON_STYLUS_PRIMARY)) != 0) {
                        conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_RIGHT);
                    }
                    else {
                        conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_RIGHT);
                    }
                }

                // Mouse tertiary or stylus secondary is middle click
                if ((changedButtons & (MotionEvent.BUTTON_TERTIARY | MotionEvent.BUTTON_STYLUS_SECONDARY)) != 0) {
                    if ((buttonState & (MotionEvent.BUTTON_TERTIARY | MotionEvent.BUTTON_STYLUS_SECONDARY)) != 0) {
                        conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_MIDDLE);
                    }
                    else {
                        conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_MIDDLE);
                    }
                }

                if (prefConfig.mouseNavButtons) {
                    if ((changedButtons & MotionEvent.BUTTON_BACK) != 0) {
                        if ((buttonState & MotionEvent.BUTTON_BACK) != 0) {
                            conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_X1);
                        }
                        else {
                            conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_X1);
                        }
                    }

                    if ((changedButtons & MotionEvent.BUTTON_FORWARD) != 0) {
                        if ((buttonState & MotionEvent.BUTTON_FORWARD) != 0) {
                            conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_X2);
                        }
                        else {
                            conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_X2);
                        }
                    }
                }

                // Handle stylus presses
                if (event.getPointerCount() == 1 && event.getActionIndex() == 0) {
                    if (event.getActionMasked() == MotionEvent.ACTION_DOWN) {
                        if (event.getToolType(0) == MotionEvent.TOOL_TYPE_STYLUS) {
                            lastAbsTouchDownTime = event.getEventTime();
                            lastAbsTouchDownX = event.getX(0) + getStreamViewOffsetX(view);
                            lastAbsTouchDownY = event.getY(0) + getStreamViewOffsetY(view);

                            // Stylus is left click
                            conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_LEFT);
                        } else if (event.getToolType(0) == MotionEvent.TOOL_TYPE_ERASER) {
                            lastAbsTouchDownTime = event.getEventTime();
                            lastAbsTouchDownX = event.getX(0) + getStreamViewOffsetX(view);
                            lastAbsTouchDownY = event.getY(0) + getStreamViewOffsetY(view);

                            // Eraser is right click
                            conn.sendMouseButtonDown(MouseButtonPacket.BUTTON_RIGHT);
                        }
                    }
                    else if (event.getActionMasked() == MotionEvent.ACTION_UP || event.getActionMasked() == MotionEvent.ACTION_CANCEL) {
                        if (event.getToolType(0) == MotionEvent.TOOL_TYPE_STYLUS) {
                            lastAbsTouchUpTime = event.getEventTime();
                            lastAbsTouchUpX = event.getX(0) + getStreamViewOffsetX(view);
                            lastAbsTouchUpY = event.getY(0) + getStreamViewOffsetY(view);

                            // Stylus is left click
                            conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_LEFT);
                        } else if (event.getToolType(0) == MotionEvent.TOOL_TYPE_ERASER) {
                            lastAbsTouchUpTime = event.getEventTime();
                            lastAbsTouchUpX = event.getX(0) + getStreamViewOffsetX(view);
                            lastAbsTouchUpY = event.getY(0) + getStreamViewOffsetY(view);

                            // Eraser is right click
                            conn.sendMouseButtonUp(MouseButtonPacket.BUTTON_RIGHT);
                        }
                    }
                }

                lastButtonState = buttonState;
            }
            // This case is for fingers
            else
            {
                if ((virtualController != null &&
                        (virtualController.getControllerMode() == VirtualController.ControllerMode.MoveButtons ||
                         virtualController.getControllerMode() == VirtualController.ControllerMode.ResizeButtons)) ||
                        (modernController != null && modernController.isEditing())) {
                    // Ignore presses when the virtual controller is being configured
                    return true;
                }

                if (event.getActionMasked() == MotionEvent.ACTION_DOWN) {
                    int mode = touchMode();
                    touchGestureIgnored = mode == TouchMode.OFF ||
                            (TouchMode.isRelative(mode) && !isInTrackpadArea(view, event));
                }
                if (touchGestureIgnored) {
                    if (event.getActionMasked() == MotionEvent.ACTION_UP ||
                            event.getActionMasked() == MotionEvent.ACTION_CANCEL) {
                        touchGestureIgnored = false;
                    }
                    return true;
                }

                if (nativeTouchEnabled && !nativeTouchUnsupported) {
                    if (event.getActionMasked() == MotionEvent.ACTION_DOWN) {
                        nativeTouchGestureActive = true;
                    }
                    if (!nativeTouchGestureActive) {
                        return true;
                    }
                    if (trySendTouchEvent(view, event)) {
                        if (event.getActionMasked() == MotionEvent.ACTION_UP ||
                                event.getActionMasked() == MotionEvent.ACTION_CANCEL) {
                            nativeTouchGestureActive = false;
                        }
                        return true;
                    }
                    nativeTouchGestureActive = false;
                    nativeTouchUnsupported = true;
                    hostNotice(HostNotices.Notice.NATIVE_TOUCH_UNAVAILABLE, R.string.stream_native_touch_fallback);
                }

                boolean rightTap = prefConfig.touchscreenTrackpad &&
                        twoFingerTap.onTouch(event, getResources().getDisplayMetrics().density);
                if (handleFingerTapGesture(event)) return true;

                // If this is the parent view, we'll offset our coordinates to appear as if they
                // are relative to the StreamView like our StreamView touch events are.
                float xOffset, yOffset;
                if (view != streamView && !prefConfig.touchscreenTrackpad) {
                    xOffset = getStreamViewOffsetX(view);
                    yOffset = getStreamViewOffsetY(view);
                }
                else {
                    xOffset = 0.f;
                    yOffset = 0.f;
                }

                int actionIndex = event.getActionIndex();

                int eventX = (int)(event.getX(actionIndex) + xOffset);
                int eventY = (int)(event.getY(actionIndex) + yOffset);

                TouchContext context = getTouchContext(actionIndex);
                if (context == null) {
                    return false;
                }

                switch (event.getActionMasked())
                {
                case MotionEvent.ACTION_POINTER_DOWN:
                case MotionEvent.ACTION_DOWN:
                    for (TouchContext touchContext : touchContextMap) {
                        touchContext.setPointerCount(event.getPointerCount());
                    }
                    context.touchDownEvent(eventX, eventY, event.getEventTime(), true);
                    break;
                case MotionEvent.ACTION_POINTER_UP:
                case MotionEvent.ACTION_UP:
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU && (event.getFlags() & MotionEvent.FLAG_CANCELED) != 0) {
                        context.cancelTouch();
                    }
                    else {
                        context.touchUpEvent(eventX, eventY, event.getEventTime());
                    }

                    for (TouchContext touchContext : touchContextMap) {
                        touchContext.setPointerCount(event.getPointerCount() - 1);
                    }
                    if (actionIndex == 0 && event.getPointerCount() > 1 && !context.isCancelled()) {
                        // The original secondary touch now becomes primary
                        context.touchDownEvent(
                                (int)(event.getX(1) + xOffset),
                                (int)(event.getY(1) + yOffset),
                                event.getEventTime(), false);
                    }
                    break;
                case MotionEvent.ACTION_MOVE:
                    // ACTION_MOVE is special because it always has actionIndex == 0
                    // We'll call the move handlers for all indexes manually
                    // First process the historical events
                    for (int i = 0; i < event.getHistorySize(); i++) {
                        for (TouchContext aTouchContextMap : touchContextMap) {
                            if (aTouchContextMap.getActionIndex() < event.getPointerCount())
                            {
                                aTouchContextMap.touchMoveEvent(
                                        (int)(event.getHistoricalX(aTouchContextMap.getActionIndex(), i) + xOffset),
                                        (int)(event.getHistoricalY(aTouchContextMap.getActionIndex(), i) + yOffset),
                                        event.getHistoricalEventTime(i));
                            }
                        }
                    }

                    // Now process the current values
                    for (TouchContext aTouchContextMap : touchContextMap) {
                        if (aTouchContextMap.getActionIndex() < event.getPointerCount())
                        {
                            aTouchContextMap.touchMoveEvent(
                                    (int)(event.getX(aTouchContextMap.getActionIndex()) + xOffset),
                                    (int)(event.getY(aTouchContextMap.getActionIndex()) + yOffset),
                                    event.getEventTime());
                        }
                    }
                    break;
                case MotionEvent.ACTION_CANCEL:
                    for (TouchContext aTouchContext : touchContextMap) {
                        aTouchContext.cancelTouch();
                        aTouchContext.setPointerCount(0);
                    }
                    break;
                default:
                    return false;
                }
                if (rightTap) clickRightMouseButton();
            }

            // Handled a known source
            return true;
        }

        // Unknown class
        return false;
    }

    @Override
    public boolean onGenericMotionEvent(MotionEvent event) {
        return handleMotionEvent(null, event) || super.onGenericMotionEvent(event);

    }

    private void updateMousePosition(View touchedView, MotionEvent event) {
        float eventX = event.getX(0) + getStreamViewOffsetX(touchedView);
        float eventY = event.getY(0) + getStreamViewOffsetY(touchedView);

        if (event.getPointerCount() == 1 && event.getActionIndex() == 0 &&
                (event.getToolType(0) == MotionEvent.TOOL_TYPE_ERASER ||
                event.getToolType(0) == MotionEvent.TOOL_TYPE_STYLUS))
        {
            switch (event.getActionMasked()) {
                case MotionEvent.ACTION_DOWN:
                case MotionEvent.ACTION_HOVER_ENTER:
                case MotionEvent.ACTION_HOVER_EXIT:
                case MotionEvent.ACTION_HOVER_MOVE:
                    if (event.getEventTime() - lastAbsTouchUpTime <= STYLUS_UP_DEAD_ZONE_DELAY &&
                            Math.sqrt(Math.pow(eventX - lastAbsTouchUpX, 2) + Math.pow(eventY - lastAbsTouchUpY, 2)) <= STYLUS_UP_DEAD_ZONE_RADIUS) {
                        // Enforce a small deadzone between touch up and hover or touch down to allow more precise double-clicking
                        return;
                    }
                    break;

                case MotionEvent.ACTION_MOVE:
                case MotionEvent.ACTION_UP:
                    if (event.getEventTime() - lastAbsTouchDownTime <= STYLUS_DOWN_DEAD_ZONE_DELAY &&
                            Math.sqrt(Math.pow(eventX - lastAbsTouchDownX, 2) + Math.pow(eventY - lastAbsTouchDownY, 2)) <= STYLUS_DOWN_DEAD_ZONE_RADIUS) {
                        // Enforce a small deadzone between touch down and move or touch up to allow more precise double-clicking
                        return;
                    }
                    break;
            }
        }

        // We may get values slightly outside our view region on ACTION_HOVER_ENTER and ACTION_HOVER_EXIT.
        // Normalize these to the view size. We can't just drop them because we won't always get an event
        // right at the boundary of the view, so dropping them would result in our cursor never really
        // reaching the sides of the screen.
        eventX = Math.min(Math.max(eventX, 0), streamView.getWidth());
        eventY = Math.min(Math.max(eventY, 0), streamView.getHeight());

        conn.sendMousePosition((short)eventX, (short)eventY, (short)streamView.getWidth(), (short)streamView.getHeight());
    }

    @Override
    public boolean onGenericMotion(View view, MotionEvent event) {
        return handleMotionEvent(view, event);
    }

    // Fingers beside the per-game touch controls reach the stream as one gesture of their own.
    private boolean onTouchControlsPassthrough(View view, MotionEvent event) {
        if (event.getActionMasked() == MotionEvent.ACTION_DOWN && isDebugOverlay() &&
                performanceOverlayScroll.getVisibility() == View.VISIBLE) {
            float x = event.getX() + gameTouchControls.getX(), y = event.getY() + gameTouchControls.getY();
            if (x >= performanceOverlayScroll.getX() && x < performanceOverlayScroll.getX() + performanceOverlayScroll.getWidth() &&
                    y >= performanceOverlayScroll.getY() && y < performanceOverlayScroll.getY() + performanceOverlayScroll.getHeight()) {
                // The debug overlay below scrolls on its own.
                return false;
            }
        }
        return onOverlayTouch(gameTouchControls, event);
    }

    @SuppressLint("ClickableViewAccessibility")
    private boolean onOverlayTouch(View view, MotionEvent event) {
        MotionEvent translated = MotionEvent.obtain(event);
        translated.offsetLocation(getStreamViewOffsetX(view), getStreamViewOffsetY(view));
        try {
            return onTouch(streamView, translated);
        } finally {
            translated.recycle();
        }
    }

    @SuppressLint("ClickableViewAccessibility")
    @Override
    public boolean onTouch(View view, MotionEvent event) {
        noteStreamPress(view, event);
        if (desktopGestures != null && desktopGestures.enabled() && foreground && connected && streamMenu == null) {
            MotionEvent relative = MotionEvent.obtain(event);
            relative.offsetLocation(getStreamViewOffsetX(view), getStreamViewOffsetY(view));
            boolean handled;
            try {
                handled = desktopGestures.onTouch(relative, grabbedInput &&
                        TouchMode.isRelative(touchMode()) && isInTrackpadArea(view, event));
            }
            finally { relative.recycle(); }
            if (handled) {
                if (trackpadPointer != null) trackpadPointer.syncCursor();
                return true;
            }
        }
        if (event.getAction() == MotionEvent.ACTION_DOWN) {
            // Tell the OS not to buffer input events for us
            //
            // NB: This is still needed even when we call the newer requestUnbufferedDispatch()!
            view.requestUnbufferedDispatch(event);
        }

        return handleMotionEvent(view, event);
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
        releaseDesktopInput();
        if (trackpadPointer != null) trackpadPointer.stopMomentum();
        rememberMicrophoneMute();
        cancelAutomaticRecovery();
        if (gameTouchControls != null) gameTouchControls.releaseInput();
        stopHardwareMetricsPolling();
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
        stopHostCursorShapes();
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
        final boolean transientFailure = StreamRecoveryPolicy.transientTermination(errorCode);
        // A renderer failure on this device says nothing about the network
        final int portTestResult = errorCode == MoonBridge.ML_ERROR_GRACEFUL_TERMINATION ||
                errorCode == MoonBridge.CLIENT_ERROR_VULKAN_RENDERER ||
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
                if (!foreground && transientFailure) rememberStreamForResume();
                // Let the display go to sleep now
                getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);

                // Stop processing controller input
                if (controllerHandler != null) controllerHandler.stop();

                // Ungrab input
                setInputGrabState(false);

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
                                    message = getResources().getString(R.string.no_frame_received_error);
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
                if (prefConfig.disableWarnings) {
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

    private void updateGameTouchProfile() {
        if (gameTouchControls == null || app == null || app.getRole() != NvApp.Role.STREAM ||
                app.isControlAction() || recoveryAppId == 0) return;
        gameTouchControls.setProfile(com.limelight.binding.input.TouchLayout.profileKey(
                getIntent().getStringExtra(EXTRA_PC_UUID), recoveryAppUuid, recoveryAppId));
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
                connectedAt = SystemClock.elapsedRealtime();
                bitrateHandler.removeCallbacks(recoveryTimeout);
                recoveryAppId = app.getControl() == NvApp.Control.NONE ? app.getAppId() : conn.getHostDetails().runningGameId;
                recoveryAppUuid = app.getControl() == NvApp.Control.NONE ? app.getAppUuid() : conn.getHostDetails().runningGameUuid;
                updateGameTouchProfile();
                controllerHandler.resendControllerArrivals();
                updateHardwareMetricsPolling();
                LimeLog.info("Stream connected");
                connecting = false;
                if (getIntent().getIntExtra(EXTRA_RECOVERY_ATTEMPTS, 0) == 0) {
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
                requestTextInput();
                startClipboardSync();
                updateHostCursorShapes();
                if (!conn.canSendInput()) {
                    if (gameTouchControls != null) gameTouchControls.hideForPip(true);
                    if (virtualController != null) {
                        virtualController.hide();
                    }
                    if (modernController != null) modernController.hide();
                    if (micButton != null) micButton.setSuppressed(true);
                    controllerHandler.stop();
                    setInputGrabState(false);
                }
                if (app.isSmallStreamInputOnly() && externalPresentation == null) {
                    ((TextView) findViewById(R.id.inputOnlyStatus)).setText(getString(
                            R.string.input_only_small_stream_status, InputOnlyPolicy.estimatedKbps() / 1000f));
                }
                findViewById(R.id.inputOnlyStatus).setVisibility(
                        app.getRole() == NvApp.Role.INPUT_ONLY || externalPresentation != null ? View.VISIBLE : View.GONE);
                // A link test before the launch may have started the stream lower than the setting.
                currentBitrate = app.isSmallStreamInputOnly() ? prefConfig.bitrate : conn.getStartBitrateKbps();
                warnIfBelowPyroWaveFloor(currentBitrate);
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
                            setInputGrabState(true);
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
                    .setMessage(confirmation.isTerminate() ? R.string.launch_confirm_terminate : R.string.launch_confirm_replace)
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

    // The reason HDR was left off although the setting is on: the status line each stream
    // and a message once per PC (notice does both).
    private void refuseHdr(HdrPolicy.Refusal refusal) {
        int text;
        switch (refusal) {
            case ANDROID_TOO_OLD: text = R.string.stream_hdr_android_unsupported; break;
            case MODE_WITHOUT_HDR10: text = R.string.stream_hdr_mode_unsupported; break;
            case DECODER_WITHOUT_HDR10: text = R.string.stream_hdr_decoder_unsupported; break;
            case DISPLAY_WITHOUT_HDR10: text = R.string.stream_hdr_display_unsupported; break;
            case HLG_NEEDS_DIRECT_OUTPUT: text = R.string.stream_hdr_hlg_needs_direct_output; break;
            default: return;
        }
        LimeLog.warning("HDR requested but not used: " + refusal);
        hostNotice(HostNotices.Notice.HDR_UNAVAILABLE, text);
    }

    private void hostNotice(HostNotices.Notice notice, int text) {
        messages.notice(getIntent().getStringExtra(EXTRA_PC_UUID), notice, getString(text));
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
            if (performanceOverlayView != null) updatePerformanceOverlay();
        });
    }

    @Override
    public void rateChange(RateChangeResult result) {
        runOnUiThread(() -> onRateChange(result));
    }

    @Override
    public void textInput(int kind, int inputType, int flags, int element) {
        runOnUiThread(() -> onHostTextField(kind, inputType, flags, element));
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
                long remaining = StreamRecoveryPolicy.WINDOW_MS -
                        (SystemClock.elapsedRealtime() - recoveryPolicy.startedAt());
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
                videoSurface.isValid() && !prefConfig.useTextureView && !useArr) {
            voteSurfaceFrameRate(videoSurface);
        }
    }

    private void voteSurfaceFrameRate(Surface surface) {
        if (useArr && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            // A zero vote lets Android infer cadence from the presented buffers.
            surface.setFrameRate(0, Surface.FRAME_RATE_COMPATIBILITY_DEFAULT,
                    Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS);
            appSurfaceFrameRateVote = 0f;
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
        messages.status("refresh", null);
        messages.status("hdr", null);
    }

    // When the screen has stayed off the rate voted for, says why on the overlay's status line.
    private void checkRefreshSwitch() {
        if (refreshSwitchMonitor == null) return;
        String text = refreshSwitchMonitor.poll(SystemClock.uptimeMillis(), getVideoDisplay(),
                surfaceCreated && !useArr ? appSurfaceFrameRateVote : null, appSurfaceSeamlessOnly);
        if (messages.status("refresh", text)) updatePerformanceOverlay();
    }

    // Render through Vulkan can't keep up on this display: say so once per PC and on the status line.
    // The renderer stays on and the setting stays; turning it off is the user's choice.
    private void checkVulkanPace() {
        if (decoderRenderer != null && decoderRenderer.takeVulkanBehind()) {
            hostNotice(HostNotices.Notice.VULKAN_BEHIND, R.string.stream_vulkan_behind);
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
            int res = hdrStatusText(status);
            if (status == HdrPolicy.Status.SHOWN_AS_SDR) {
                text = getString(res, HdrFormatPolicy.baseLayerName(decoderRenderer.getAnnouncedHdrFormat()));
            } else if (res != 0) {
                text = getString(res);
            }
            // Plain words, once per PC, for what the Vulkan renderer cannot show of this screen's HDR.
            int limit = vulkanHdrLimitText(decoderRenderer.takeVulkanHdrLimit());
            if (limit != 0) hostNotice(HostNotices.Notice.VULKAN_HDR_LIMIT, limit);
            // Once per stream on the status line (and once per PC as a message): the output window
            // refused HDR10+, so the picture keeps HDR10's static metadata alone.
            if (decoderRenderer.takeVulkanHdr10PlusRefused()) {
                LimeLog.warning("HDR10+ refused by the Vulkan renderer's window");
                hostNotice(HostNotices.Notice.VULKAN_HDR10_PLUS_REFUSED, R.string.stream_hdr_vulkan_no_hdr10_plus);
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
                hostNotice(HostNotices.Notice.SDR10_HOST_HDR, R.string.stream_sdr10_host_hdr);
            } else if (step == Sdr10Policy.HostHdrStep.REARM) {
                // The PC's display is in SDR now; if it goes to HDR again, say so again.
                HostNotices.reset(this, getIntent().getStringExtra(EXTRA_PC_UUID), HostNotices.Notice.SDR10_HOST_HDR);
            }
            if (tenBit && report != null) sdr10LastHostHdr = report;
        }
        if (messages.status("hdr", text)) updatePerformanceOverlay();
    }

    static int hdrStatusText(HdrPolicy.Status status) {
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
            applySurfaceFrameRate(videoSurface, baseSurfaceFrameRate(), Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS);
            LimeLog.info("Content frame rate vote withdrawn");
            logDisplayRefreshPolicy(getVideoDisplay());
            return;
        }
        float contentHz = contentRateVote.next(now, contentMilliHz);
        if (contentHz <= 0) return;
        float vote = com.limelight.binding.video.DisplayFrameRatePolicy.contentVote(contentHz,
                baseSurfaceFrameRate(), mayReduceRefreshRate(), panelMaxRefreshRate);
        applySurfaceFrameRate(videoSurface, vote, Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS);
        LimeLog.info("Content frame rate vote: " + vote + " Hz (game at " + contentHz + " Hz)");
        logDisplayRefreshPolicy(getVideoDisplay());
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
            rememberStreamForResume();
            displayedFailureDialog = true;
            stopConnection();
            // The stopped decoder cannot be reused. Resume the captured game in a fresh
            // activity when foreground returns, with the existing recovery budget.
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
        conn.sendMouseMove(
                (short)mouseDeltaX.scale(deltaX, prefConfig.mouseSpeed / 100f),
                (short)mouseDeltaY.scale(deltaY, prefConfig.mouseSpeed / 100f));
    }

    @Override
    public void mouseButtonEvent(int buttonId, boolean down) {
        byte buttonIndex;

        switch (buttonId)
        {
        case EvdevListener.BUTTON_LEFT:
            buttonIndex = MouseButtonPacket.BUTTON_LEFT;
            break;
        case EvdevListener.BUTTON_MIDDLE:
            buttonIndex = MouseButtonPacket.BUTTON_MIDDLE;
            break;
        case EvdevListener.BUTTON_RIGHT:
            buttonIndex = MouseButtonPacket.BUTTON_RIGHT;
            break;
        case EvdevListener.BUTTON_X1:
            buttonIndex = MouseButtonPacket.BUTTON_X1;
            break;
        case EvdevListener.BUTTON_X2:
            buttonIndex = MouseButtonPacket.BUTTON_X2;
            break;
        default:
            LimeLog.warning("Unhandled button: "+buttonId);
            return;
        }

        if (down) {
            conn.sendMouseButtonDown(buttonIndex);
        }
        else {
            conn.sendMouseButtonUp(buttonIndex);
        }
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
        if (Looper.myLooper() != Looper.getMainLooper()) {
            runOnUiThread(() -> keyboardEvent(buttonDown, keyCode));
            return;
        }
        short keyMap = keyboardTranslator.translate(keyCode, -1);
        if (keyMap != 0) {
            // handleSpecialKeys() takes the Android keycode
            if (handleSpecialKeys(keyCode, buttonDown)) {
                return;
            }
            if (!grabbedInput) {
                return;
            }

            if (buttonDown) {
                sendKeyboardInput(keyMap, KeyboardPacket.KEY_DOWN, getModifierState(keyCode), (byte)0);
            }
            else {
                sendKeyboardInput(keyMap, KeyboardPacket.KEY_UP, getModifierState(keyCode), (byte)0);
            }
        }
    }

    private void sendKeyboardInput(short key, byte direction, byte modifiers, byte flags) {
        synchronized (pressedKeys) {
            if (direction == KeyboardPacket.KEY_DOWN) {
                pressedKeys.put(key, flags);
            } else {
                pressedKeys.remove(key);
            }
            if (!customPressedKeys.containsKey(key)) {
                conn.sendKeyboardInput(key, direction, (byte)(modifiers | customModifierState()), flags);
            }
        }
    }

    private byte customModifierState() {
        int modifiers = 0;
        for (int key : customPressedKeys.values()) {
            int mask = getModifierMask(key);
            modifiers |= mask | (mask >> 8);
        }
        return (byte)modifiers;
    }

    private void sendCustomKey(int keyCode, boolean down) {
        short key = keyboardTranslator.translate(keyCode, -1);
        synchronized (pressedKeys) {
            if (down) customPressedKeys.put(key, keyCode); else customPressedKeys.remove(key);
            if (!pressedKeys.containsKey(key)) {
                conn.sendKeyboardInput(key, down ? KeyboardPacket.KEY_DOWN : KeyboardPacket.KEY_UP,
                        (byte)(getModifierState() | customModifierState()), (byte)0);
            }
        }
    }

    private void sendCustomMouse(int button, boolean down) {
        conn.sendOverlayMouseButton((byte)button, down);
    }

    private void releaseKeyboardInput() {
        if (gameTouchControls != null) gameTouchControls.releaseInput();
        // Sticky modifiers and held keys of the PC keyboard go up with everything else.
        if (pcKeyboard != null) pcKeyboard.releaseInputs();
        pcKeyboardKeys.clear();
        pcKeyboardAxisDevice = -1;
        synchronized (pressedKeys) {
            for (Map.Entry<Short, Byte> key : pressedKeys.entrySet()) {
                conn.sendKeyboardInput(key.getKey(), KeyboardPacket.KEY_UP, (byte) 0, key.getValue());
            }
            pressedKeys.clear();
        }
        releaseDesktopInput();
        modifierFlags = 0;
        specialKeyCode = KeyEvent.KEYCODE_UNKNOWN;
        waitingForAllModifiersUp = false;
        getWindow().getDecorView().removeCallbacks(toggleGrab);
    }

    private void releaseMouseButtons() {
        if (connected) {
            // Evdev, stylus and synthetic Back clicks do not share the MotionEvent button mask.
            byte[] buttons = {MouseButtonPacket.BUTTON_LEFT, MouseButtonPacket.BUTTON_RIGHT, MouseButtonPacket.BUTTON_MIDDLE,
                    MouseButtonPacket.BUTTON_X1, MouseButtonPacket.BUTTON_X2};
            for (byte button : buttons) {
                conn.sendMouseButtonUp(button);
            }
        }
        lastButtonState = 0;
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
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (!connected || isFinishing() || isDestroyed()) {
                    return;
                }
                String displayLine = displayRefreshOverlayLine();
                String networkText = !isAdaptiveBitrateEnabled() ? network : network + "\n" +
                        getString(R.string.stream_auto_bitrate_status, currentBitrate / 1000.0);
                if (adaptiveQuality != null) {
                    AdaptiveQualityController.Target quality = adaptiveQuality.current();
                    networkText += "\n" + getString(R.string.quality_status,
                            getResources().getStringArray(R.array.adaptive_quality_names)[qualityMode().ordinal()],
                            quality.width, quality.height, quality.fps);
                } else if (qualityStatus != null) {
                    networkText += "\n" + qualityStatus;
                }
                MicrophoneStatus micStatus = conn.getMicrophoneStatus();
                if (micStatus.label != 0) {
                    networkText += "\n" + getString(micStatus.label);
                }
                for (String note : messages.notes()) {
                    if (!note.equals(qualityStatus)) networkText += "\n" + note;
                }
                networkText += "\n" + wifiText(streamHealth.stallPeriodMs());
                debugPerformanceText = PerformanceOverlay.debugText(Game.this, video + displayLine, networkText, decode);
                statisticsPerformanceText = statisticsText;
                compactPerformanceText = compactText;
                updatePerformanceOverlay();
            }
        });
    }

    private String displayRefreshOverlayLine() {
        if (!prefConfig.vrr) return "";
        Display display = getVideoDisplay();
        float modeRefreshRate = Build.VERSION.SDK_INT >= Build.VERSION_CODES.M ?
                display.getMode().getRefreshRate() : display.getRefreshRate();
        return "\n" + getString(R.string.perf_overlay_vrr, modeRefreshRate, display.getRefreshRate(),
                useArr ? "ARR" : getString(R.string.perf_overlay_vrr_max_hz));
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
                return handleKeyMultiple(keyEvent);
            default:
                return false;
        }
    }
}
