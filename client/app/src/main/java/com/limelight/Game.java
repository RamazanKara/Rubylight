package com.limelight;

import com.limelight.binding.input.ControllerHandler;
import com.limelight.binding.input.KeyboardTranslator;
import com.limelight.binding.input.capture.InputCaptureProvider;
import com.limelight.binding.input.driver.UsbDriverService;
import com.limelight.binding.input.evdev.EvdevListener;
import com.limelight.binding.input.pad.PointerMove;
import com.limelight.binding.input.touch.TrackpadPointer;
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
import com.limelight.nvstream.AdaptiveBitrateController;
import com.limelight.nvstream.AdaptiveQualityController;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.NvConnectionListener;
import com.limelight.nvstream.PyroWaveBitrateController;
import com.limelight.nvstream.RateChangeResult;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.LaunchConfirmation;
import com.limelight.nvstream.http.NvApp;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.GameGestures;
import com.limelight.ui.HostCursorShapes;
import com.limelight.ui.KeyboardOverlay;
import com.limelight.ui.LegacyKeyboardInsets;
import com.limelight.ui.PcKeyboardOverlay;
import com.limelight.ui.StreamOrientation;
import com.limelight.ui.StreamSafeAreaController;
import com.limelight.ui.StreamView;
import com.limelight.ui.VideoScaling;
import com.limelight.utils.UiHelper;

import android.annotation.SuppressLint;
import android.annotation.TargetApi;
import android.app.Activity;
import android.app.PendingIntent;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.ServiceConnection;
import android.content.SharedPreferences;
import android.content.res.Configuration;
import android.graphics.SurfaceTexture;
import android.hardware.display.DisplayManager;
import android.hardware.input.InputManager;
import android.net.wifi.WifiManager;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
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
import android.widget.TextView;

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
    final GameStagedExit stagedExitController = new GameStagedExit(this);
    private final GameOnscreenControls onscreenControls = new GameOnscreenControls(this);
    private final GameStreamLoops streamLoops = new GameStreamLoops(this);
    private final GameLifecycle lifecycle = new GameLifecycle(this);
    private final RumbleLogFilter rumbleLogFilter = new RumbleLogFilter();

    static final class RumbleLogFilter {
        private static final int FIXED_SLOTS = 16;
        private final long[] motors = new long[FIXED_SLOTS];
        private final long[] triggers = new long[FIXED_SLOTS];
        private java.util.HashMap<Short, Long> extraMotors;
        private java.util.HashMap<Short, Long> extraTriggers;

        RumbleLogFilter() {
            java.util.Arrays.fill(motors, -1L);
            java.util.Arrays.fill(triggers, -1L);
        }

        synchronized boolean shouldLogMotor(short controller, short low, short high) {
            return record(motors, false, controller, low, high);
        }

        synchronized boolean shouldLogTriggers(short controller, short left, short right) {
            return record(triggers, true, controller, left, right);
        }

        private boolean record(long[] slots, boolean isTriggers, short controller, short first, short second) {
            long packed = (((long) first & 0xFFFFL) << 16) | ((long) second & 0xFFFFL);
            if (controller >= 0 && controller < slots.length) {
                if (slots[controller] == packed) return false;
                slots[controller] = packed;
                return true;
            }
            java.util.HashMap<Short, Long> map = isTriggers ? extraTriggers : extraMotors;
            if (map == null) {
                map = new java.util.HashMap<>();
                if (isTriggers) extraTriggers = map; else extraMotors = map;
            }
            Long prev = map.put(controller, packed);
            return prev == null || prev != packed;
        }
    }

    TrackpadPointer trackpadPointer;
    // Set while the host sends its cursor shapes for the Local cursor; read on the control thread.
    volatile HostCursorShapes hostCursorShapes;
    boolean nativeTouchEnabled;
    // The modern controller's pointer touchpad, in the host's pixels as the screen touchpad is.
    final PointerMove pointerMove = new PointerMove();

    ControllerHandler controllerHandler;
    KeyboardTranslator keyboardTranslator;
    KeyboardOverlay keyboardOverlay;
    PcKeyboardOverlay pcKeyboard;
    final InputManager.InputDeviceListener pcKeyboardDeviceListener = streamLoops.pcKeyboardDeviceListener();
    // The notice that Controllers is Off while a controller is connected, once a stream (GameConnection).
    com.limelight.preferences.ControllersChoice.OffNotice controllersOff;

    boolean desktopWindowFocused = true;
    boolean dispatchingDesktopTouch;
    // The on-screen controller, built the first time the controls are shown or their layout is edited.
    com.limelight.binding.input.pad.ModernController modernController;
    // An external gamepad is connected; with Hide while a controller is connected the on-screen controls leave.
    volatile boolean physicalControllerConnected;
    com.limelight.ui.MicButton micButton;
    com.limelight.ui.MenuButton menuButton;

    PreferenceConfiguration prefConfig;
    SharedPreferences tombstonePrefs;

    NvConnection conn;
    com.limelight.ui.ConnectingScreen spinner;
    boolean displayedFailureDialog = false;
    boolean connecting = false;
    // The second screen's stream has been started beside this one (DualScreen).
    boolean dualScreenStarted;
    // Names this first stream to its second (DualScreen.EXTRA_FIRST_STREAM).
    String dualScreenFirst;
    // Receives the other screen's stream's messages (DualScreen.link).
    BroadcastReceiver dualScreenLink;
    volatile boolean connected = false;
    boolean autoEnterPip = false;
    com.limelight.ui.AdaptiveLayout adaptiveLayout;
    boolean surfaceCreated = false;
    boolean attemptedConnection = false;
    volatile Thread connectionStopThread;
    int recoveryAttempts;
    long recoveryStartedAt;
    int recoveryAppId;
    String recoveryAppUuid;
    Runnable pendingRecovery;
    boolean recoveryHandoffInProgress;
    // Set when the last automatic recovery was refused because its attempts or window ran out.
    boolean recoveryBudgetExhausted;
    boolean recoveryCancelledOnPause;
    Intent stoppedStreamResumeIntent;
    final Runnable recoveryTimeout = streamLoops::runRecoveryTimeout;
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
    final WindowFocusActionQueue windowFocusActions = new WindowFocusActionQueue();
    final Runnable runWindowFocusActions = streamLoops::runWindowFocusActions;
    int currentBitrate;
    // A bitrate the player set in the stream menu this session (0: none), which an automatic reconnect keeps
    // (GameRecovery.putSessionBitrate). Automatic bitrate stays off while it is set.
    int sessionManualBitrate;
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
    final Runnable updateRateChangeBitrate = streamLoops::runUpdateRateChangeBitrate;
    // Gives up on 0x553C requests the host left unanswered for StreamRateChanges.ANSWER_MS, loudly.
    final Runnable expireRateChanges = streamLoops::runExpireRateChanges;
    java.util.concurrent.ExecutorService streamHealthThread;
    // The quality tick, whole, through the decoder's door: ticks pause while a rebuild sets the decoder up again.
    final Runnable updateQuality = () -> videoOutput.whenDecoderReady("tick", streamLoops::runQualityTick);
    final Runnable updateBitrate = streamLoops::runUpdateBitrate;

    String pcName;
    String appName;
    NvApp app;
    float desiredRefreshRate;
    float panelMaxRefreshRate;
    // Mid-stream reconfigure (0x5532): a stream at the phone's native resolution, or at the
    // panel's top refresh rate, follows fold, window and panel changes without reconnecting.
    boolean nativeResolutionStream;
    // Any size but a preset, a custom one too: on a squarish screen the window keeps the shape the
    // stream was asked in (StreamOrientation.requested).
    boolean keepsRequestedShape;
    boolean fpsFollowsDisplay;
    int streamFps;
    // The size the decoder was set up for (the negotiated one) or a reconfigure changed it to; 0 until then.
    int streamWidth, streamHeight;
    float streamPanelHz;
    // The Orientation setting, and whether the host draws this stream at the shape asked for (its
    // virtual display), so a turned window can ask for a turned stream.
    int streamOrientation = StreamOrientation.AUTO;
    boolean hostShapesStream;
    int lastWindowOrientation = Configuration.ORIENTATION_UNDEFINED;
    final StreamReconfigureRequester reconfigureRequester = new StreamReconfigureRequester();
    final Runnable checkReconfigure = () -> videoOutput.whenDecoderReady("check", videoOutput::checkStreamReconfigure);
    final DisplayManager.DisplayListener inputDisplayListener = streamLoops.inputDisplayListener();
    // Whether this stream asked for HDR (setting on, HDR10 display and decoder).
    boolean streamHdr;
    // 10-bit formats offered for 10-bit SDR (the setting), without asking the PC for HDR.
    boolean streamSdr10;
    DisplayCapsReporter displayCapsReporter;

    boolean useArr;
    // The last successful vote made by this activity, not a measurement of the display's rate.
    Float appSurfaceFrameRateVote;
    PendingIntent pipDisconnectIntent;
    final BroadcastReceiver pipReceiver = streamLoops.pipReceiver();
    InputCaptureProvider inputCaptureProvider;
    int modifierFlags = 0;
    boolean grabbedInput = true;
    boolean cursorVisible = false;
    StreamView streamView;
    final StreamSafeAreaController safeArea = new StreamSafeAreaController(this);
    StreamView renderView;
    com.limelight.ui.StreamPresentation externalPresentation;
    com.limelight.ui.DesktopGestures desktopGestures;
    com.limelight.ui.GameTouchControls gameTouchControls;
    boolean outputSwitchPending;
    Surface videoSurface;

    boolean isHidingOverlays;
    TextView notificationOverlayView;
    int requestedNotificationOverlayVisibility = View.GONE;
    TextView performanceOverlayView;
    View performanceOverlayScroll;
    // What the host last said about holding the bitrate down for the network; null when it has not.
    LinkNoticeStatus linkNotice;
    // The stream said once that the PC lowered the bitrate (GameAdaptation.noteLinkLowered).
    boolean linkLoweredTold;
    // The PC keeps this stream's bitrate: Never's takeover, the bitrate dialog's Keep my bitrate or a bitrate the
    // player chose in the stream went out (any bitrate change makes it stand back). GameAdaptation.keepBitrateFromMenu.
    boolean bitrateKept;
    // The bitrate the player last chose in this stream, which Keep my bitrate keeps; 0 for none.
    int playerBitrateKbps;
    // The bitrate this stream started with (negotiated), which Keep my bitrate keeps when the player chose none.
    int streamStartBitrateKbps;
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
    /** Join requests and guests of this device's game, on hosts that offer them. */
    StreamGuests streamGuests;
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

    // Experimental: on Android TV, an HDR stream ends in stages (see StagedStreamExit / GameStagedExit).
    final StagedStreamExit stagedExit = new StagedStreamExit();
    // True from the first finish() of a staged exit on; isFinishing() reports it so every
    // "is this stream ending" check in this class holds while the exit is staged.
    volatile boolean exiting;
    boolean exitFinished;

    WifiManager.WifiLock highPerfWifiLock;
    WifiManager.WifiLock lowLatencyWifiLock;

    boolean usbDriverServiceBound;
    boolean usbDriverReleased;
    UsbDriverService.UsbDriverBinder usbDriverBinder;
    ServiceConnection usbDriverServiceConnection = streamLoops.usbDriverServiceConnection();
    java.util.concurrent.ScheduledExecutorService nativeMenuThread;
    final Runnable hideSystemUi = streamLoops::runHideSystemUi;
    final Runnable toggleGrab = () -> touchInput.setInputGrabState(!grabbedInput);
    private boolean backByController;

    public static final String EXTRA_HOST = "Host";
    public static final String EXTRA_PORT = "Port";
    public static final String EXTRA_HTTPS_PORT = "HttpsPort";
    public static final String EXTRA_APP_NAME = "AppName";
    public static final String EXTRA_APP_ID = "AppId";
    public static final String EXTRA_APP_UUID = "AppUuid";
    /** The cover's name in the art cache, for the connecting screen. */
    public static final String EXTRA_APP_ART_KEY = "AppArtKey";
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

    /** Says that Controllers is Off, the one time it is due with {@code connected} controllers here. */
    void sayControllersOff(int connected) {
        if (controllersOff != null && !isFinishing() && controllersOff.take(connected)) {
            messages.follow(getString(R.string.stream_controllers_off));
        }
    }

    @Override
    public boolean isFinishing() {
        return exiting || super.isFinishing();
    }

    @Override
    public void finish() {
        if (stagedExitController.finish()) super.finish();
    }

    void finishSuper() {
        super.finish();
    }

    // For a hand-off to a new stream activity (reconnect, new intent; GameRecovery.handOff): the stream is not ending.
    void finishNow() {
        exitFinished = true;
        super.finish();
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        com.limelight.ui.AppTheme.apply(this);
        super.onCreate(savedInstanceState);
        // A hand-off in split screen starts this screen with CLEAR_TASK (GameRecovery.handOff). The copies of this
        // intent that later starts make (the next hand-off, the second screen) must not clear a task themselves.
        getIntent().setFlags(getIntent().getFlags() & ~Intent.FLAG_ACTIVITY_CLEAR_TASK);
        // One connection to a PC at a time: a stream ends Microphone only, whose close it then waits for.
        com.limelight.mic.MicOnlyService.stop(this);
        streamConnection.onCreate(savedInstanceState);
        // Settings › Show motion readings; does nothing while it is off.
        com.limelight.binding.input.GyroDiagnosticOverlay.attach(this);
    }

    static boolean isValidLaunch(String host, int port, String uniqueId, int appId, String appUuid) {
        return host != null && !host.trim().isEmpty() && port > 0 && port <= 65535
                && uniqueId != null && !uniqueId.isEmpty()
                && (appId != StreamConfiguration.INVALID_APP_ID || (appUuid != null && !appUuid.isEmpty()));
    }

    @Override
    public void onConfigurationChanged(Configuration newConfig) {
        super.onConfigurationChanged(newConfig);
        lifecycle.onConfigurationChanged(newConfig);
    }

    public void setMetaKeyCaptureState(boolean enabled) { keyboardInput.setMetaKeyCaptureState(enabled); }

    /** Split screen, a freeform window or picture-in-picture: the stream does not fill the screen. */
    boolean inWindow() { return isInMultiWindowMode() || isInPictureInPictureMode(); }

    void reapplySystemKeys() { lifecycle.reapplySystemKeys(); }

    @Override
    public void onUserLeaveHint() {
        super.onUserLeaveHint();
        lifecycle.onUserLeaveHint();
    }

    @Override
    @TargetApi(Build.VERSION_CODES.R)
    public boolean onPictureInPictureRequested() {
        return lifecycle.onPictureInPictureRequested();
    }

    @Override
    public boolean dispatchTouchEvent(MotionEvent event) {
        // A direct press can reach a split-screen window before its focus notification.
        // Only callbacks inside this dispatch may use that press as evidence of interaction.
        boolean previous = dispatchingDesktopTouch;
        dispatchingDesktopTouch = lifecycle.isDesktopPointerTouch(event);
        try {
            return super.dispatchTouchEvent(event);
        } finally {
            dispatchingDesktopTouch = previous;
        }
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        lifecycle.onWindowFocusChanged(hasFocus);
    }

    @Override public boolean isWindowedStream() { return videoOutput.isWindowedStream(); }
    protected boolean windowCoversDisplay() { return videoOutput.windowCoversDisplay(); }
    @Override public boolean usesFullDisplayForVideo() { return videoOutput.usesFullDisplayForVideo(); }
    @Override public boolean keepsCornersClear() { return prefConfig != null && VideoScaling.keepsCornersClear(prefConfig.videoScaling); }

    @Override
    public void onMultiWindowModeChanged(boolean isInMultiWindowMode) {
        super.onMultiWindowModeChanged(isInMultiWindowMode);
        lifecycle.onMultiWindowModeChanged(isInMultiWindowMode);
    }

    @Override
    protected void onDestroy() {
        com.limelight.binding.input.GyroDiagnosticOverlay.detach(this);
        lifecycle.onDestroy();
        super.onDestroy();
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        lifecycle.onNewIntent(intent);
    }

    @Override
    protected void onStart() {
        super.onStart();
        // Stopping this screen stops the stream, so a pairing link waits as long as it is up.
        com.limelight.preferences.LinkGate.hold(this);
        if (adaptiveLayout != null) adaptiveLayout.start();
    }

    @Override
    protected void onResume() {
        super.onResume();
        lifecycle.onResume();
    }

    @Override
    protected void onPause() {
        lifecycle.onPause();
        super.onPause();
    }

    @Override
    protected void onStop() {
        super.onStop();
        // Home, or the screen going off; a finishing screen has been through finish or is handing off.
        if (!isFinishing() && !isChangingConfigurations()) streamConnection.playerLeft();
        com.limelight.preferences.LinkGate.release(this, this);
        lifecycle.onStopAfterLinkGate();
    }

    @Override
    public void onBackPressed() {
        onBack(backByController);
    }

    void onBack(boolean byController) {
        if (!lifecycle.onBack(byController)) super.onBackPressed();
    }

    @Override
    protected void attachBaseContext(Context base) {
        super.attachBaseContext(UiHelper.localeContext(base));
    }

    public void leaveStreamFromController() { menuActions.leaveStreamFromController(); }
    public void showStreamMenu() { menuActions.showStreamMenu(false); }
    /** A controller, remote or key opened the menu: its first tile shows focus at once, so the next press moves it. */
    public void showStreamMenuFromController() { menuActions.showStreamMenu(true); }
    public void steerStreamMenu(int keyCode) { menuActions.steerStreamMenu(keyCode); }

    static StringBuilder healthText(Context context, com.limelight.utils.StreamHealth.Report report, String connection) {
        return GameStatusText.healthText(context, report, connection);
    }

    static boolean healthTrialAllowed(boolean adaptiveBitrate, boolean rateChangeBitrate, boolean adaptiveQuality) {
        return GameStatusText.healthTrialAllowed(adaptiveBitrate, rateChangeBitrate, adaptiveQuality);
    }

    boolean onscreenControlsShown() { return onscreenControls.onscreenControlsShown(); }
    void setPhysicalControllerConnected(boolean connected) { onscreenControls.setPhysicalControllerConnected(connected); }
    void onGameButtonsEditing() { onscreenControls.onGameButtonsEditing(); }
    void hideOnscreenControllers() { onscreenControls.hideOnscreenControllers(); }
    void showOnscreenController() { onscreenControls.showOnscreenController(); }
    void ensureOnscreenControllers() { onscreenControls.ensureOnscreenControllers(); }

    interface RouteProbe { String problem(String host, int port); }

    static String rateChangeText(Context context, RateChangeResult result) {
        return GameStatusText.rateChangeText(context, result);
    }

    static String rateChangeReason(Context context, int reason) {
        return GameStatusText.rateChangeReason(context, reason);
    }

    @Override public boolean onKeyDown(int keyCode, KeyEvent event) { return handleKeyDown(event) || super.onKeyDown(keyCode, event); }
    @Override public boolean handleKeyDown(KeyEvent event) { return keyboardInput.handleKeyDown(event); }

    @Override
    public boolean onKeyUp(int keyCode, KeyEvent event) {
        if (handleKeyUp(event)) return true;
        // A Back from a controller, D-pad or remote opens the menu focused, as Guide does; the touch Back does not.
        backByController = keyCode == KeyEvent.KEYCODE_BACK && (event.isFromSource(InputDevice.SOURCE_GAMEPAD)
                || event.isFromSource(InputDevice.SOURCE_DPAD) || event.isFromSource(InputDevice.SOURCE_JOYSTICK));
        try {
            return super.onKeyUp(keyCode, event);
        } finally {
            backByController = false;
        }
    }

    @Override public boolean handleKeyUp(KeyEvent event) { return keyboardInput.handleKeyUp(event); }
    @Override public boolean onKeyMultiple(int keyCode, int repeatCount, KeyEvent event) {
        return keyboardInput.handleKeyMultiple(event) || super.onKeyMultiple(keyCode, repeatCount, event);
    }
    @Override public boolean handleTextInput(CharSequence text) { return keyboardInput.handleTextInput(text); }
    @Override public void toggleKeyboard() { keyboardInput.toggleKeyboard(); }
    @Override public boolean onGenericMotionEvent(MotionEvent event) { return touchInput.handleMotionEvent(null, event) || super.onGenericMotionEvent(event); }
    @Override public boolean onGenericMotion(View view, MotionEvent event) { return touchInput.handleMotionEvent(view, event); }
    @SuppressLint("ClickableViewAccessibility")
    @Override public boolean onTouch(View view, MotionEvent event) { return touchInput.onTouch(view, event); }

    @Override public void stageStarting(final String stage) { streamConnection.stageStarting(stage); }

    @Override
    public void stageComplete(String stage) {
        // The host has accepted this stream's launch or resume: the second screen need not wait for the
        // rest of the handshake and the first frame. The user can leave before this reaches the UI.
        runOnUiThread(() -> {
            if (isFinishing() || isDestroyed()) return;
            DualScreen.start(this);
        });
    }

    @Override public void stageFailed(final String stage, final int portFlags, final int errorCode) { streamConnection.stageFailed(stage, portFlags, errorCode); }

    public static boolean isDecoderStageFailure(DecoderSetupFailure decoderFailure, String stage) {
        return GameStatusText.isDecoderStageFailure(decoderFailure, stage);
    }

    public static String startupFailureText(Context context, DecoderSetupFailure decoderFailure, String stage,
                                            String connectionErrorMessage, int errorCode) {
        return GameStatusText.startupFailureText(context, decoderFailure, stage, connectionErrorMessage, errorCode);
    }

    static String terminationDescription(int errorCode) { return GameStatusText.terminationDescription(errorCode); }
    static String errorCodeText(int errorCode) { return GameStatusText.errorCodeText(errorCode); }

    @Override public void connectionTerminated(final int reportedErrorCode) { streamConnection.connectionTerminated(reportedErrorCode); }
    @Override public void connectionStatusUpdate(final int connectionStatus) { streamConnection.connectionStatusUpdate(connectionStatus); }
    @Override public void streamNetworkLost() { streamConnection.streamNetworkLost(); }
    @Override public void streamNetworkReturned(final int change, final boolean metered) { streamConnection.streamNetworkReturned(change, metered); }
    @Override public void connectionStarted() { streamConnection.connectionStarted(); }
    @Override public void automaticStartStopped() { streamConnection.automaticStartStopped(); }
    @Override public void displayMessage(final String message) { streamConnection.displayMessage(message); }
    @Override public void streamMoved(String device) { streamConnection.streamMoved(device); }
    @Override public void launchConfirmationRequired(LaunchConfirmation confirmation) { streamConnection.launchConfirmationRequired(confirmation); }
    @Override public void launchConfirmationFinished() { streamConnection.launchConfirmationFinished(); }
    @Override public void launchActionCompleted(String message) { streamConnection.launchActionCompleted(message); }
    @Override public void displayTransientMessage(final String message) { streamConnection.displayTransientMessage(message); }

    @Override
    public void rumble(short controllerNumber, short lowFreqMotor, short highFreqMotor) {
        if (rumbleLogFilter.shouldLogMotor(controllerNumber, lowFreqMotor, highFreqMotor)) {
            LimeLog.info(String.format((Locale)null, "Rumble on gamepad %d: %04x %04x", controllerNumber, lowFreqMotor, highFreqMotor));
        }
        controllerHandler.handleRumble(controllerNumber, lowFreqMotor, highFreqMotor);
    }

    @Override
    public void rumbleTriggers(short controllerNumber, short leftTrigger, short rightTrigger) {
        if (rumbleLogFilter.shouldLogTriggers(controllerNumber, leftTrigger, rightTrigger)) {
            LimeLog.info(String.format((Locale)null, "Rumble on gamepad triggers %d: %04x %04x", controllerNumber, leftTrigger, rightTrigger));
        }
        controllerHandler.handleRumbleTriggers(controllerNumber, leftTrigger, rightTrigger);
    }

    @Override
    public void setHdrMode(boolean enabled, byte[] hdrMetadata) {
        LimeLog.info("Display HDR mode: " + (enabled ? "enabled" : "disabled"));
        decoderRenderer.setHdrMode(enabled, hdrMetadata);
    }

    @Override public void setMotionEventState(short controllerNumber, byte motionType, short reportRateHz) { controllerHandler.handleSetMotionEventState(controllerNumber, motionType, reportRateHz); }
    @Override public void setControllerLED(short controllerNumber, byte r, byte g, byte b) { controllerHandler.handleSetControllerLED(controllerNumber, r, g, b); }

    @Override
    public void linkNotice(int state, int ceilingKbps, int currentKbps) {
        LinkNoticeStatus status = LinkNoticeStatus.parse(state, ceilingKbps, currentKbps);
        runOnUiThread(() -> {
            linkNotice = status;
            // A cut before the PC's answer to the bitrate query: that answer may hold it.
            if (status != null) adaptation.pcLowered();
            adaptation.noteLinkLowered(status);
            if (performanceOverlayView != null) overlays.updatePerformanceOverlay();
        });
    }

    @Override public void rateChange(RateChangeResult result) { runOnUiThread(() -> adaptation.onRateChange(result)); }
    @Override public void textInput(int kind, int inputType, int flags, int element) { runOnUiThread(() -> keyboardInput.onHostTextField(kind, inputType, flags, element)); }
    @Override public void clipboardSync(String text) { runOnUiThread(() -> { if (clipboardSync != null) clipboardSync.received(text); }); }
    @Override public void clipboardSyncRefused(int reason) { runOnUiThread(() -> { if (clipboardSync != null) clipboardSync.refused(reason); }); }

    @Override
    public void cursorShape(byte[] message) {
        HostCursorShapes shapes = hostCursorShapes;
        if (shapes != null) shapes.accept(message);
    }

    @Override public void controllerAudio(int kind, byte[] payload) { controllerHandler.handleControllerAudio(kind, payload); }
    @Override public void controllerHaptics(short controllerNumber, int sequence, byte[] pcm) { controllerHandler.handleControllerHaptics(controllerNumber, sequence, pcm); }

    @Override
    public void setAdaptiveTriggers(short controllerNumber, byte eventFlags, byte typeLeft,
                                    byte typeRight, byte[] left, byte[] right) {
        controllerHandler.handleSetAdaptiveTriggers(controllerNumber, eventFlags, typeLeft, typeRight, left, right);
    }

    @Override public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) { streamConnection.startConnectionOnSurface(); }
    @Override public void surfaceCreated(SurfaceHolder holder) { streamConnection.configureVideoSurface(holder.getSurface()); }
    @Override public void surfaceDestroyed(SurfaceHolder holder) { streamConnection.destroyVideoSurface(); }

    static int hdrStatusText(HdrPolicy.Status status) { return hdrStatusText(status, false, false); }
    static int hdrStatusText(HdrPolicy.Status status, boolean pyroWaveHostUpdate, boolean pyroWaveHlgRefused) {
        return GameStatusText.hdrStatusText(status, pyroWaveHostUpdate, pyroWaveHlgRefused);
    }
    static int vulkanHdrLimitText(HdrFormatPolicy.DrawnLimit limit, boolean forFrameWarp) {
        return GameStatusText.vulkanHdrLimitText(limit, forFrameWarp);
    }
    static int pyroWaveHdrLimitText(HdrFormatPolicy.DrawnLimit limit) {
        return GameStatusText.pyroWaveHdrLimitText(limit);
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

    @Override public void onSurfaceTextureUpdated(SurfaceTexture texture) {}
    @Override public void mouseMove(int deltaX, int deltaY) { touchInput.mouseMove(deltaX, deltaY); }
    @Override public void mouseButtonEvent(int buttonId, boolean down) { touchInput.mouseButtonEvent(buttonId, down); }
    @Override public void mouseVScroll(byte amount) { conn.sendMouseScroll(amount); }
    @Override public void mouseHScroll(byte amount) { conn.sendMouseHScroll(amount); }
    @Override public void keyboardEvent(boolean buttonDown, short keyCode) { keyboardInput.keyboardEvent(buttonDown, keyCode); }

    @Override
    public void onSystemUiVisibilityChange(int visibility) {
        if (!connected) return;
        if ((visibility & View.SYSTEM_UI_FLAG_FULLSCREEN) == 0 || (visibility & View.SYSTEM_UI_FLAG_HIDE_NAVIGATION) == 0) {
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
            case KeyEvent.ACTION_DOWN: return handleKeyDown(keyEvent);
            case KeyEvent.ACTION_UP: return handleKeyUp(keyEvent);
            case KeyEvent.ACTION_MULTIPLE: return keyboardInput.handleKeyMultiple(keyEvent);
            default: return false;
        }
    }
}
