package com.limelight;


import com.limelight.binding.input.ControllerHandler;
import com.limelight.binding.input.KeyboardTranslator;
import com.limelight.binding.input.capture.InputCaptureProvider;
import com.limelight.binding.input.touch.TrackpadPointer;
import com.limelight.binding.input.driver.UsbDriverService;
import com.limelight.binding.input.evdev.EvdevListener;
import com.limelight.binding.input.virtual_controller.VirtualController;
import com.limelight.binding.video.DecoderSetupFailure;
import com.limelight.binding.video.DisplayCapsReporter;
import com.limelight.binding.video.HdrFormatPolicy;
import com.limelight.binding.video.HdrPolicy;
import com.limelight.binding.video.LinkNoticeStatus;
import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.PerfOverlayListener;
import com.limelight.binding.video.PerformanceOverlay;
import com.limelight.binding.video.StagedStreamExit;
import com.limelight.binding.video.UpscalingPolicy;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.AdaptiveBitrateController;
import com.limelight.nvstream.AdaptiveQualityController;
import com.limelight.nvstream.PyroWaveBitrateController;
import com.limelight.nvstream.RateChangeResult;
import com.limelight.nvstream.NvConnectionListener;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.LaunchConfirmation;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.GameGestures;
import com.limelight.ui.HostCursorShapes;
import com.limelight.ui.KeyboardOverlay;
import com.limelight.ui.LegacyKeyboardInsets;
import com.limelight.ui.PcKeyboardOverlay;
import com.limelight.ui.StreamOrientation;
import com.limelight.ui.StreamView;
import com.limelight.utils.Dialog;
import com.limelight.utils.SpinnerDialog;
import com.limelight.utils.UiHelper;

import android.annotation.SuppressLint;
import android.annotation.TargetApi;
import android.app.Activity;
import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.core.view.WindowInsetsControllerCompat;
import android.app.PendingIntent;
import android.content.BroadcastReceiver;
import com.limelight.ui.PictureInPicturePolicy;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.SharedPreferences;
import android.content.res.Configuration;
import android.graphics.SurfaceTexture;
import android.hardware.input.InputManager;
import android.net.wifi.WifiManager;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.os.IBinder;
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
import android.widget.FrameLayout;
import android.widget.TextView;
import android.widget.Toast;

import java.io.IOException;
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
    final GameMenuActions menuActions = new GameMenuActions(this);
    final GameAdaptation adaptation = new GameAdaptation(this);
    final GameRecovery streamRecovery = new GameRecovery(this);
    final GameVideoOutput videoOutput = new GameVideoOutput(this);
    final GameConnection streamConnection = new GameConnection(this);

    TrackpadPointer trackpadPointer;
    // Set while the host sends its cursor shapes for the Local cursor; read on the control thread.
    volatile HostCursorShapes hostCursorShapes;
    boolean nativeTouchEnabled;

    ControllerHandler controllerHandler;
    KeyboardTranslator keyboardTranslator;
    KeyboardOverlay keyboardOverlay;
    PcKeyboardOverlay pcKeyboard;
    final InputManager.InputDeviceListener pcKeyboardDeviceListener = new InputManager.InputDeviceListener() {
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
    SharedPreferences tombstonePrefs;

    NvConnection conn;
    SpinnerDialog spinner;
    boolean displayedFailureDialog = false;
    boolean connecting = false;
    volatile boolean connected = false;
    boolean autoEnterPip = false;
    com.limelight.ui.AdaptiveLayout adaptiveLayout;
    boolean surfaceCreated = false;
    boolean attemptedConnection = false;
    volatile Thread connectionStopThread;
    StreamRecoveryPolicy recoveryPolicy = new StreamRecoveryPolicy(0, 0);
    int recoveryAppId;
    String recoveryAppUuid;
    Runnable pendingRecovery;
    boolean recoveryHandoffInProgress;
    // Set when the last automatic recovery was refused because its attempts or window ran out.
    boolean recoveryBudgetExhausted;
    boolean recoveryCancelledOnPause;
    Intent stoppedStreamResumeIntent;
    final Runnable recoveryTimeout = () -> {
        if (this.connecting && this.foreground && !isFinishing() && streamRecovery.isRecoveryAttempt()) {
            displayedFailureDialog = true;
            streamConnection.stopConnection();
            if (spinner != null) { spinner.dismiss(); spinner = null; }
            streamRecovery.showReconnectDialog(R.string.conn_terminated_title, getString(R.string.stream_recovery_exhausted));
        }
    };
    int suppressPipRefCount = 0;
    android.app.Dialog streamMenu;
    Runnable streamMenuAction;
    LaunchConfirmation pendingConfirmation;
    String connectionErrorMessage;
    boolean hostActionInProgress;
    // Set while this device asks the host to end the session. Hosts drop the stream while
    // handling that request, often without an end message, so any termination is expected.
    volatile boolean hostExitRequested;
    boolean foreground;
    int foregroundGeneration;
    boolean restoreInputAfterMenu;
    private boolean restoreInputOnResume;
    final WindowFocusActionQueue windowFocusActions = new WindowFocusActionQueue();
    final Runnable runWindowFocusActions = () -> {
        if (!foreground || !connected || isFinishing()) {
            windowFocusActions.clear();
        } else if (streamMenu == null) {
            windowFocusActions.runPending(hasWindowFocus(), SystemClock.uptimeMillis());
        }
    };
    int currentBitrate;
    AdaptiveBitrateController adaptiveBitrate;
    PyroWaveBitrateController pyroWaveBitrate;
    boolean poorConnection;
    long adaptiveBitrateRetryAt;
    int adaptiveBitrateRetryTarget;
    // Automatic quality switched auto bitrate on for itself; the person's own choice was off.
    boolean qualityAutoBitrate;
    AdaptiveQualityController adaptiveQuality;
    AdaptiveQualityController.Target pendingQualityTarget;
    long qualityRequestSentAt;
    String qualityStatus;
    int qualityMaxWidth, qualityMaxHeight, qualityMaxFps;
    int qualityMaxFrameRateMilliHz;
    final Handler bitrateHandler = new Handler(Looper.getMainLooper());
    final com.limelight.utils.StreamHealth streamHealth = new com.limelight.utils.StreamHealth();
    com.limelight.utils.WifiDiagnostics wifiDiagnostics;
    int healthOriginalBitrate, healthTrialBitrate;
    // 0x553C rate changes, for a Rust stream whose host offered them; null otherwise.
    StreamRateChanges rateChanges;
    // Automatic bitrate on this device: one client-hint sample a second, as a 0x553C request.
    final Runnable updateRateChangeBitrate = new Runnable() {
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
            adaptation.scheduleRateChangeExpiry();
            bitrateHandler.postDelayed(this, 1000);
        }
    };
    // Gives up on 0x553C requests the host left unanswered for StreamRateChanges.ANSWER_MS, loudly.
    final Runnable expireRateChanges = new Runnable() {
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
            adaptation.scheduleRateChangeExpiry();
        }
    };
    java.util.concurrent.ExecutorService streamHealthThread;

    final Runnable updateQuality = new Runnable() {
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
                    adaptation.stopAdaptiveQualityWithError();
                    return;
                }
            } else {
                long rtt = MoonBridge.getEstimatedRttInfo();
                AdaptiveQualityController.Target target = adaptiveQuality.sample(now,
                        decoderRenderer.hasRecentVideoFrames(now), poorConnection,
                        decoderRenderer.getNetworkFrameLossPercent(), decoderRenderer.getDecodeTimeMs(),
                        rtt == -1 ? -1 : (float) (rtt & 0xffffffffL), decoderRenderer.getQueueDelayMs(),
                        adaptation.adaptationThermalLevel(),
                        adaptiveBitrate != null && adaptiveBitrate.hasRoomBeforeQualityReduction(),
                        decoderRenderer.takeDecodeTiming(streamFps));
                if (target != null && !adaptation.applyAdaptiveQuality(target, now)) return;
            }
            bitrateHandler.postDelayed(this, 1000);
        }
    };

    final Runnable updateBitrate = new Runnable() {
        @Override
        public void run() {
            if (!connected || !adaptation.isAdaptiveBitrateEnabled()) {
                return;
            }
            if (!hostActionInProgress && streamMenu == null && SystemClock.uptimeMillis() >= adaptiveBitrateRetryAt) {
                if (adaptiveBitrateRetryAt != 0) {
                    adaptation.applyAdaptiveBitrate(adaptiveBitrateRetryTarget);
                } else {
                    long now = SystemClock.uptimeMillis();
                    long rttInfo = MoonBridge.getEstimatedRttInfo();
                    float jitterMs = rttInfo == -1 ? -1 : (float) (rttInfo & 0xffffffffL);
                    int thermalLevel = adaptation.adaptationThermalLevel();
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
                        adaptation.applyAdaptiveBitrate(target);
                    }
                }
            } else {
                if (adaptiveBitrate != null) adaptiveBitrate.suspend();
                if (pyroWaveBitrate != null) pyroWaveBitrate.suspend();
            }
            bitrateHandler.postDelayed(this, 1000);
        }
    };
    String pcName;
    String appName;
    NvApp app;
    float desiredRefreshRate;
    float panelMaxRefreshRate;
    // Mid-stream reconfigure (0x5532): a stream at the phone's native resolution, or at the
    // panel's top refresh rate, follows fold, window and panel changes without reconnecting.
    boolean nativeResolutionStream;
    boolean fpsFollowsDisplay;
    int streamFps;
    float streamPanelHz;
    // The Orientation setting, and whether the host draws this stream at the shape asked for (its
    // virtual display), so a turned window can ask for a turned stream.
    int streamOrientation = StreamOrientation.AUTO;
    boolean hostShapesStream;
    int lastWindowOrientation = Configuration.ORIENTATION_UNDEFINED;
    final StreamReconfigureRequester reconfigureRequester = new StreamReconfigureRequester();
    final Runnable checkReconfigure = videoOutput::checkStreamReconfigure;
    final android.hardware.display.DisplayManager.DisplayListener inputDisplayListener =
            new android.hardware.display.DisplayManager.DisplayListener() {
                @Override public void onDisplayAdded(int displayId) {}
                @Override public void onDisplayRemoved(int displayId) {
                    Display video = videoOutput.getVideoDisplay();
                    if (video.getDisplayId() == displayId) streamConnection.noteVideoDisplay(video, true);
                    if (externalPresentation != null && externalPresentation.getDisplay().getDisplayId() == displayId) {
                        videoOutput.returnVideoToDevice();
                    }
                }
                @Override public void onDisplayChanged(int displayId) {
                    Display display = videoOutput.getVideoDisplay();
                    if (display.getDisplayId() == displayId) streamConnection.noteVideoDisplay(display, false);
                    if (conn != null && display.getDisplayId() == displayId) {
                        videoOutput.updateInputPollingRate();
                        // Also fires when a foldable moves the stream to its other screen.
                        videoOutput.reportDisplayCaps();
                        videoOutput.onDisplayModesChanged();
                    }
                }
            };
    // Whether this stream asked for HDR (setting on, HDR10 display and decoder).
    boolean streamHdr;
    // 10-bit formats offered for 10-bit SDR (the setting), without asking the PC for HDR.
    boolean streamSdr10;
    DisplayCapsReporter displayCapsReporter;

    boolean useArr;
    // The last successful vote made by this activity, not a measurement of the display's rate.
    Float appSurfaceFrameRateVote;
    PendingIntent pipDisconnectIntent;
    final BroadcastReceiver pipReceiver = new BroadcastReceiver() {
        @Override public void onReceive(Context context, Intent intent) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
                    PictureInPicturePolicy.canDisconnect(connected, isInPictureInPictureMode(), app.getRole())) {
                menuActions.leaveStream();
            }
        }
    };
    InputCaptureProvider inputCaptureProvider;
    int modifierFlags = 0;
    boolean grabbedInput = true;
    boolean cursorVisible = false;
    StreamView streamView;
    StreamView renderView;
    com.limelight.ui.StreamPresentation externalPresentation;
    com.limelight.ui.DesktopGestures desktopGestures;
    com.limelight.ui.GameTouchControls gameTouchControls;
    boolean outputSwitchPending;
    Surface videoSurface;

    boolean isHidingOverlays;
    private boolean wasInPictureInPicture;
    TextView notificationOverlayView;
    int requestedNotificationOverlayVisibility = View.GONE;
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
    com.limelight.binding.usb.UsbForwarding usbForwarding;
    // Clipboard sync (0x5508) while this stream's PC runs it.
    com.limelight.utils.ClipboardSync clipboardSync;
    final Runnable showStreamMenuHint = overlays::showStreamMenuHint;

    MediaCodecDecoderRenderer decoderRenderer;
    boolean reportedCrash;
    // The PC (or this device) ended the stream. A decoder failure after that is the stream going
    // away, not a decoder crash.
    volatile boolean streamTerminated;
    // Experimental: a TV that loses its HDMI signal takes the decoder down with it. Whether the
    // video display is off, invalid or gone, as of its last change.
    volatile boolean videoDisplayOff;

    // Experimental: on Android TV, an HDR stream ends in stages (stop and release the decoder and
    // video output, let the display settle, give the display mode back, settle again, then finish)
    // so the HDR to SDR switch and the mode revert never overlap and wedge the TV's HDMI link.
    // Everything else finishes at once, as always. See StagedStreamExit.
    final StagedStreamExit stagedExit = new StagedStreamExit();
    private final Handler exitHandler = new Handler(Looper.getMainLooper());
    private final Runnable stagedExitRunnable = this::stagedExitTick;
    // True from the first finish() of a staged exit on; isFinishing() reports it so every
    // "is this stream ending" check in this class holds while the exit is staged.
    volatile boolean exiting;
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
    void finishNow() {
        exitFinished = true;
        super.finish();
    }

    private boolean stagedExitApplies() {
        return !isDestroyed() && attemptedConnection && decoderRenderer != null &&
                StagedStreamExit.applies(streamConnection.isAndroidTv(),
                        streamHdr || Boolean.TRUE.equals(decoderRenderer.getHostHdrReport()));
    }

    private void beginStagedExit() {
        if (!stagedExit.begin(SystemClock.uptimeMillis())) return;
        exiting = true;
        LimeLog.info("Exit: staged (Android TV, HDR stream, Experimental)");
        touchInput.setInputGrabState(false);
        if (streamMenu != null) streamMenu.dismiss();
        LimeLog.info("Exit: stopping the connection and releasing the decoder");
        streamConnection.stopConnection();
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
                android.view.Window window = videoOutput.getVideoWindow();
                WindowManager.LayoutParams attributes = window.getAttributes();
                attributes.preferredDisplayModeId = videoOutput.getVideoDisplay().getMode().getModeId();
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
            android.view.Window window = videoOutput.getVideoWindow();
            WindowManager.LayoutParams attributes = window.getAttributes();
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) attributes.preferredDisplayModeId = 0;
            attributes.preferredRefreshRate = 0;
            window.setAttributes(attributes);
        } catch (RuntimeException e) {
            LimeLog.warning("Exit: could not restore the display mode: " + e);
        }
    }

    WifiManager.WifiLock highPerfWifiLock;
    WifiManager.WifiLock lowLatencyWifiLock;

    boolean usbDriverServiceBound;
    UsbDriverService.UsbDriverBinder usbDriverBinder;
    ServiceConnection usbDriverServiceConnection = new ServiceConnection() {
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
    // The network the stream started on (StreamLayers.Network), kept across reconnects and handoffs.
    static final String EXTRA_STREAM_NETWORK = "StreamNetwork";
    static final String EXTRA_RECOVERY_MEDIA_FAILURE = "RecoveryMediaFailure";
    // Packed full chroma failed in this session and the reconnect runs without it (not the setting).
    static final String EXTRA_PACKED_OFF = "PackedChromaOff";
    static final String EXTRA_MICROPHONE_MUTED = "rubylight.microphoneMuted";
    // When the user returned from that trip: marks the resume and starts its own deadline.
    static final String EXTRA_TRIP_RESUME_STARTED = "rubylight.tripResumeStarted";

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        com.limelight.ui.AppTheme.apply(this);
        super.onCreate(savedInstanceState);
        streamConnection.onCreate(savedInstanceState);
    }

    static boolean isValidLaunch(String host, int port, String uniqueId, int appId, String appUuid) {
        return host != null && !host.trim().isEmpty() && port > 0 && port <= 65535
                && uniqueId != null && !uniqueId.isEmpty()
                && (appId != StreamConfiguration.INVALID_APP_ID || (appUuid != null && !appUuid.isEmpty()));
    }

    @Override
    public void onConfigurationChanged(Configuration newConfig) {
        super.onConfigurationChanged(newConfig);
        videoOutput.configureStreamWindow();
        if (controllerHandler == null || isFinishing()) {
            return;
        }

        // Set requested orientation for possible new screen size
        videoOutput.setPreferredOrientationForCurrentDisplay();
        // Attaching a hardware keyboard or switching the IME's language changes the layout.
        keyboardInput.reportKeyboardLayout();

        // A fold, rotation or window change may change the native size
        boolean turned = newConfig.orientation != lastWindowOrientation;
        lastWindowOrientation = newConfig.orientation;
        if (turned) videoOutput.noteRotationWithoutReconfigure();
        videoOutput.scheduleReconfigureCheck(StreamOrientation.settleMs(streamOrientation, hostShapesStream, turned,
                StreamReconfigureRequester.SETTLE_MS));

        if (virtualController != null && prefConfig.onscreenController) {
            // Refresh layout of OSC for possible new screen size
            virtualController.refreshLayout();
        }
        // The modern controller and the microphone button follow their frame's size by themselves.

        videoOutput.applyVideoScaling();

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
                    enterPictureInPictureMode(videoOutput.getPictureInPictureParams(false));
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
                enterPictureInPictureMode(videoOutput.getPictureInPictureParams(false));
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

    @Override
    public boolean isWindowedStream() {
        return videoOutput.isWindowedStream();
    }

    protected boolean windowCoversDisplay() {
        return videoOutput.windowCoversDisplay();
    }

    @Override
    public boolean usesFullDisplayForVideo() {
        return videoOutput.usesFullDisplayForVideo();
    }

    @SuppressLint("InlinedApi")
    final Runnable hideSystemUi = new Runnable() {
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

    @Override
    @TargetApi(Build.VERSION_CODES.N)
    public void onMultiWindowModeChanged(boolean isInMultiWindowMode) {
        super.onMultiWindowModeChanged(isInMultiWindowMode);
        videoOutput.configureStreamWindow();
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

        videoOutput.updatePipAutoEnter();
        videoOutput.scheduleReconfigureCheck();

        // Correct the system UI visibility flags
        videoOutput.hideSystemUi(50);
    }

    @Override
    protected void onDestroy() {
        if (nativeMenuThread != null) nativeMenuThread.shutdownNow();
        if (streamHealthThread != null) streamHealthThread.shutdownNow();
        // A staged exit still waiting on the display ends here with the activity.
        exitHandler.removeCallbacksAndMessages(null);
        exitFinished = true;
        streamConnection.stopConnection();
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
        streamConnection.stopConnection();
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
        if (streamRecovery.resumeStoppedStreamIfNeeded()) {
            recoveryCancelledOnPause = false;
            restoreInputOnResume = false;
            return;
        }
        if (recoveryCancelledOnPause && !connected && !isFinishing()) {
            displayedFailureDialog = true;
            streamConnection.stopConnection();
            if (spinner != null) { spinner.dismiss(); spinner = null; }
            streamRecovery.showReconnectDialog(R.string.conn_terminated_title, getString(R.string.stream_recovery_paused));
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
        recoveryCancelledOnPause = recoveryHandoffInProgress || pendingRecovery != null || (streamRecovery.isRecoveryAttempt() && connecting);
        streamRecovery.cancelAutomaticRecovery();
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
        streamRecovery.rememberStreamForResume(true);

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
            streamConnection.stopConnection();

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

        if (streamRecovery.finishesWhenStopped()) finish();
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

    public void leaveStreamFromController() {
        menuActions.leaveStreamFromController();
    }

    public void showStreamMenu() {
        menuActions.showStreamMenu();
    }

    java.util.concurrent.ScheduledExecutorService nativeMenuThread;

    public void steerStreamMenu(int keyCode) {
        menuActions.steerStreamMenu(keyCode);
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

    interface RouteProbe { String problem(String host, int port); }

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
        streamConnection.stageStarting(stage);
    }

    @Override
    public void stageComplete(String stage) {
    }

    @Override
    public void stageFailed(final String stage, final int portFlags, final int errorCode) {
        streamConnection.stageFailed(stage, portFlags, errorCode);
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
    static String errorCodeText(int errorCode) {
        return Math.abs(errorCode) > 1000 ? Integer.toHexString(errorCode) : Integer.toString(errorCode);
    }

    @Override
    public void connectionTerminated(final int reportedErrorCode) {
        streamConnection.connectionTerminated(reportedErrorCode);
    }

    @Override
    public void connectionStatusUpdate(final int connectionStatus) {
        streamConnection.connectionStatusUpdate(connectionStatus);
    }

    @Override
    public void streamNetworkLost() {
        streamConnection.streamNetworkLost();
    }

    @Override
    public void streamNetworkReturned(final int change, final boolean metered) {
        streamConnection.streamNetworkReturned(change, metered);
    }

    @Override
    public void connectionStarted() {
        streamConnection.connectionStarted();
    }

    @Override
    public void displayMessage(final String message) {
        streamConnection.displayMessage(message);
    }

    @Override
    public void launchConfirmationRequired(LaunchConfirmation confirmation) {
        streamConnection.launchConfirmationRequired(confirmation);
    }

    @Override
    public void launchConfirmationFinished() {
        streamConnection.launchConfirmationFinished();
    }

    @Override
    public void launchActionCompleted(String message) {
        streamConnection.launchActionCompleted(message);
    }

    @Override
    public void displayTransientMessage(final String message) {
        streamConnection.displayTransientMessage(message);
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
        runOnUiThread(() -> adaptation.onRateChange(result));
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
        streamConnection.startConnectionOnSurface();
    }

    @Override
    public void surfaceCreated(SurfaceHolder holder) {
        streamConnection.configureVideoSurface(holder.getSurface());
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

    @Override
    public void surfaceDestroyed(SurfaceHolder holder) {
        streamConnection.destroyVideoSurface();
    }

    @Override
    public void onSurfaceTextureAvailable(SurfaceTexture texture, int width, int height) {
        texture.setDefaultBufferSize(prefConfig.upscalingMode == UpscalingPolicy.Mode.OFF ? prefConfig.width : width,
                prefConfig.upscalingMode == UpscalingPolicy.Mode.OFF ? prefConfig.height : height);
        streamConnection.configureVideoSurface(new Surface(texture));
        streamConnection.startConnectionOnSurface();
    }

    @Override
    public void onSurfaceTextureSizeChanged(SurfaceTexture texture, int width, int height) {
        texture.setDefaultBufferSize(prefConfig.upscalingMode == UpscalingPolicy.Mode.OFF ? prefConfig.width : width,
                prefConfig.upscalingMode == UpscalingPolicy.Mode.OFF ? prefConfig.height : height);
    }

    @Override
    public boolean onSurfaceTextureDestroyed(SurfaceTexture texture) {
        Surface surface = videoSurface;
        streamConnection.destroyVideoSurface();
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
            videoOutput.hideSystemUi(2000);
        }
        else if ((visibility & View.SYSTEM_UI_FLAG_HIDE_NAVIGATION) == 0) {
            videoOutput.hideSystemUi(2000);
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
        videoOutput.updatePipAutoEnter();
    }

    @Override
    public void onUsbPermissionPromptCompleted() {
        suppressPipRefCount--;
        videoOutput.updatePipAutoEnter();
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
