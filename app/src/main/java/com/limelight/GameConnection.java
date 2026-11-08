package com.limelight;

import com.limelight.binding.PlatformBinding;
import com.limelight.binding.audio.AndroidAudioRenderer;
import com.limelight.binding.audio.SilentAudioRenderer;
import com.limelight.binding.input.ControllerHandler;
import com.limelight.binding.input.KeyboardTranslator;
import com.limelight.binding.input.capture.InputCaptureManager;
import com.limelight.binding.input.driver.UsbDriverService;
import com.limelight.binding.video.CrashAccounting;
import com.limelight.binding.video.CrashListener;
import com.limelight.binding.video.DecoderSetupFailure;
import com.limelight.binding.video.PackedFallback;
import com.limelight.binding.video.DiscardingDecoderRenderer;
import com.limelight.binding.video.DisplayCapsReporter;
import com.limelight.binding.video.HdrPolicy;
import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.binding.video.PerformanceOverlay;
import com.limelight.binding.video.Sdr10Policy;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.computers.HostNotices;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.StreamMove;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.av.audio.AudioRenderer;
import com.limelight.nvstream.av.video.VideoDecoderRenderer;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.LaunchConfirmation;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.pin.PinGate;
import com.limelight.preferences.GlPreferences;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.LegacyKeyboardInsets;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.ui.StreamView;
import com.limelight.utils.ContinueHereGate;
import com.limelight.utils.Dialog;
import com.limelight.utils.NetworkGrace;
import com.limelight.utils.LaunchSource;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.ShortcutHelper;
import com.limelight.utils.UiHelper;
import android.app.Activity;
import androidx.appcompat.app.AlertDialog;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.hardware.input.InputManager;
import android.media.AudioManager;
import android.net.ConnectivityManager;
import android.net.wifi.WifiManager;
import android.os.Build;
import android.os.Bundle;
import android.os.SystemClock;
import android.preference.PreferenceManager;
import android.view.Display;
import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.Surface;
import android.view.TextureView;
import android.view.View;
import android.view.Window;
import android.view.WindowManager;
import android.widget.FrameLayout;
import android.widget.TextView;
import android.widget.Toast;
import java.io.ByteArrayInputStream;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;

/** The stream's connection: starting it on the video surface, the connection's stages and status, its messages and launch confirmations, losing and regaining the network, and stopping it. */
final class GameConnection {
    private final Game game;

    GameConnection(Game game) {
        this.game = game;
    }

    /** Game.onCreate after its theme and super.onCreate. */
    void onCreate(Bundle savedInstanceState) {
        if (savedInstanceState != null) {
            LimeLog.info("Stream restored after process death");
            game.messages.alert(R.string.stream_restore_help, Toast.LENGTH_LONG);
            streamFailed = true;
            game.finish();
            return;
        }
        if (!Game.isValidLaunch(game.getIntent().getStringExtra(Game.EXTRA_HOST),
                game.getIntent().getIntExtra(Game.EXTRA_PORT, NvHTTP.DEFAULT_HTTP_PORT),
                game.getIntent().getStringExtra(Game.EXTRA_UNIQUEID),
                game.getIntent().getIntExtra(Game.EXTRA_APP_ID, StreamConfiguration.INVALID_APP_ID),
                game.getIntent().getStringExtra(Game.EXTRA_APP_UUID))) {
            streamFailed = true;
            game.finish();
            return;
        }

        UiHelper.setLocale(game);

        // We don't want a title bar
        game.requestWindowFeature(Window.FEATURE_NO_TITLE);

        // Full-screen
        game.getWindow().addFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN);

        // If we're going to use immersive mode, we want to have
        // the entire screen
        game.getWindow().getDecorView().setSystemUiVisibility(
                View.SYSTEM_UI_FLAG_LAYOUT_STABLE |
                        View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION |
                        View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN);

        game.getWindow().addFlags(WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN);

        // Listen for UI visibility events
        game.getWindow().getDecorView().setOnSystemUiVisibilityChangeListener(game);

        // Change volume button behavior
        game.setVolumeControlStream(AudioManager.STREAM_MUSIC);

        // Inflate the content
        game.setContentView(R.layout.activity_game);

        // Start the spinner
        game.spinner = showConnecting(game.getString(R.string.conn_establishing_msg));

        // Read the stream preferences
        // Profiles, PC and game values are read once here and never change during the stream. A
        // reconnect or handoff keeps the network the stream started on, and so the same profile.
        com.limelight.preferences.StreamLayers.Network startNetwork = null;
        String seenNetwork = game.getIntent().getStringExtra(Game.EXTRA_STREAM_NETWORK);
        if (seenNetwork != null) {
            try {
                startNetwork = com.limelight.preferences.StreamLayers.Network.valueOf(seenNetwork);
            } catch (IllegalArgumentException ignored) {
                // Read the network again.
            }
        }
        game.prefConfig = PreferenceConfiguration.readPreferences(game, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID),
                HostCapabilityCache.load(game, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID)),
                game.getIntent().getStringExtra(Game.EXTRA_APP_UUID),
                game.getIntent().getIntExtra(Game.EXTRA_APP_ID, StreamConfiguration.INVALID_APP_ID), startNetwork);
        if (DualScreen.isSecond(game.getIntent())) {
            DualScreen.configureSecond(game.prefConfig, game.getWindowManager().getDefaultDisplay());
        }
        game.prefConfig.experimentalPacked444 = PackedFallback.effective(game.prefConfig.experimentalPacked444,
                game.getIntent().getBooleanExtra(Game.EXTRA_PACKED_OFF, false));
        game.getIntent().putExtra(Game.EXTRA_STREAM_NETWORK, game.prefConfig.layers.network.name());
        HdrPolicy.setDebugHlgOnly(BuildConfig.DEBUG, game.prefConfig.debugHdrHlgOnly);
        game.overlays.showStreamLayers(game.prefConfig, startNetwork == null);
        game.streamRecovery.readRecoveryBudget(SystemClock.elapsedRealtime());
        game.panelMaxRefreshRate = com.limelight.binding.video.DisplayFrameRatePolicy.maxRefreshRate(
                game.videoOutput.getVideoDisplay(), game.prefConfig.width, game.prefConfig.height);
        if (game.prefConfig.vrr) {
            game.prefConfig.fps = com.limelight.binding.video.DisplayFrameRatePolicy.streamFrameRate(game.panelMaxRefreshRate);
            game.prefConfig.launchRefreshRateX100 = Math.max(100, Math.min(100000, Math.round(game.panelMaxRefreshRate * 100)));
        }
        com.limelight.utils.RedactedLog.addSecret(game.getIntent().getStringExtra(Game.EXTRA_PC_NAME));
        com.limelight.utils.RedactedLog.addSecret(game.getIntent().getStringExtra(Game.EXTRA_HOST));
        LimeLog.info(LaunchSource.describe(game.getIntent()));
        LimeLog.info("Stream request: " + game.prefConfig.width + "x" + game.prefConfig.height + " at " + game.prefConfig.fps +
                " fps (resolution " + game.prefConfig.resolutionSource() +
                (game.prefConfig.vrr ? ", VRR at the panel's " + game.panelMaxRefreshRate + " Hz" : "") + ")");
        LimeLog.info("PC's own stream values: " + (game.prefConfig.pcSettings == null ? "none" : game.prefConfig.pcSettings));
        if (game.prefConfig.layers != null) {
            LimeLog.info("Stream settings layers: " + com.limelight.preferences.StreamLayers.describe(game.prefConfig.layers));
        }
        // Fixed presets keep their size; anything else is the phone's own resolution and follows it.
        game.nativeResolutionStream = game.prefConfig.resolutionFollowsScreen ||
                PreferenceConfiguration.isNativeResolution(game.prefConfig.width, game.prefConfig.height);
        game.fpsFollowsDisplay = com.limelight.binding.video.DisplayFrameRatePolicy.fpsFollowsDisplay(game.prefConfig.vrr,
                game.prefConfig.fps, com.limelight.binding.video.DisplayFrameRatePolicy.maxRefreshRate(
                        game.videoOutput.getVideoDisplay()));
        game.nativeTouchEnabled = !game.prefConfig.touchscreenTrackpad &&
                PreferenceManager.getDefaultSharedPreferences(game).getBoolean("checkbox_native_touch", false);
        game.tombstonePrefs = game.getSharedPreferences("DecoderTombstone", 0);
        game.streamOrientation = game.prefConfig.streamOrientation;
        game.lastWindowOrientation = game.getResources().getConfiguration().orientation;

        // Landscape on phones unless the Orientation setting says otherwise
        game.videoOutput.setPreferredOrientationForCurrentDisplay();

        game.videoOutput.configureStreamWindow();

        // Listen for non-touch events on the game surface
        game.streamView = game.findViewById(R.id.surfaceView);
        game.videoOutput.initializeVideoOutput();
        game.streamView.setOnGenericMotionListener(game);
        // Keeps the Local cursor pointer over the same stream pixel when the stream moves or resizes.
        game.streamView.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> {
            if (game.trackpadPointer != null) game.trackpadPointer.syncCursor();
        });
        game.streamView.setOnKeyListener(game);
        game.streamView.setInputCallbacks(game);
        game.adaptiveLayout = com.limelight.ui.AdaptiveLayout.attach(game, game.findViewById(android.R.id.content));
        game.adaptiveLayout.setInsetsObserver(game.keyboardInput::onWindowInsets);
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            game.legacyKeyboard = new LegacyKeyboardInsets(game, game.findViewById(android.R.id.content), game.keyboardInput::onKeyboardInsets);
            game.legacyKeyboard.start();
        }
        game.streamView.addOnLayoutChangeListener((view, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom) -> {
            if (game.connected) game.videoOutput.updatePipAutoEnter();
        });
        game.findViewById(android.R.id.content).addOnLayoutChangeListener(
                (view, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom) -> game.videoOutput.onPaneLaidOut(view));

        // Listen for touch events on the background touch view to enable trackpad mode
        // to work on areas outside of the StreamView itself. We use a separate View
        // for this rather than just handling it at the Activity level, because that
        // allows proper touch splitting, which the OSC relies upon.
        View backgroundTouchView = game.findViewById(R.id.backgroundTouchView);
        backgroundTouchView.setOnTouchListener(game);

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            // Request unbuffered input event dispatching for all input classes we handle here.
            // Without this, input events are buffered to be delivered in lock-step with VBlank,
            // artificially increasing input latency while streaming.
            game.streamView.requestUnbufferedDispatch(
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

        game.notificationOverlayView = game.findViewById(R.id.notificationOverlay);
        game.streamMenuHint = game.findViewById(R.id.streamMenuHint);
        // Rotation, a fold or the overlay moving to the top centre move the hint too.
        game.streamMenuHint.getViewTreeObserver().addOnGlobalLayoutListener(() -> {
            if (game.streamMenuHint.getVisibility() == View.VISIBLE) {
                com.limelight.ui.StreamMenuHint.place(game.streamMenuHint, game.performanceOverlayScroll);
            }
        });

        game.performanceOverlayView = game.findViewById(R.id.performanceOverlay);
        game.performanceOverlayScroll = game.findViewById(R.id.performanceOverlayScroll);
        com.limelight.ui.StreamGlass.dressHud(game.performanceOverlayScroll, game.streamMenuHint, game.notificationOverlayView);
        game.performanceOverlayScroll.setOnTouchListener((view, event) -> !game.overlays.isDebugOverlay() && game.touchInput.onOverlayTouch(view, event));
        game.overlayMode = PerformanceOverlay.normalizeMode(PreferenceManager.getDefaultSharedPreferences(game)
                .getString("performance_overlay_mode", PerformanceOverlay.MODE_COMPACT));
        game.overlayCorner = com.limelight.ui.OverlayPlacement.Corner.parse(game.prefConfig.overlayPosition);
        game.overlays.attachOverlayPlacement((android.view.ViewGroup) game.streamView.getParent());
        game.overlays.updatePerformanceOverlay();

        game.inputCaptureProvider = InputCaptureManager.getInputCaptureProvider(game, game);

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            game.streamView.setOnCapturedPointerListener(new View.OnCapturedPointerListener() {
                @Override
                public boolean onCapturedPointer(View view, MotionEvent motionEvent) {
                    return game.touchInput.handleMotionEvent(view, motionEvent);
                }
            });
        }

        // Warn the user if they're on a metered connection
        ConnectivityManager connMgr = (ConnectivityManager) game.getSystemService(Context.CONNECTIVITY_SERVICE);
        if (connMgr.isActiveNetworkMetered() && !game.prefConfig.disableWarnings) {
            // Streaming can use a lot of mobile data: worth acting on before it does.
            game.messages.alert(R.string.conn_metered, Toast.LENGTH_LONG);
        }

        // Make sure Wi-Fi is fully powered up
        WifiManager wifiMgr = (WifiManager) game.getApplicationContext().getSystemService(Context.WIFI_SERVICE);
        try {
            if (wifiMgr != null) {
                game.highPerfWifiLock = wifiMgr.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, BuildConfig.BRAND_NAME + " High Perf Lock");
                game.highPerfWifiLock.setReferenceCounted(false);
                game.highPerfWifiLock.acquire();

                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    game.lowLatencyWifiLock = wifiMgr.createWifiLock(WifiManager.WIFI_MODE_FULL_LOW_LATENCY, BuildConfig.BRAND_NAME + " Low Latency Lock");
                    game.lowLatencyWifiLock.setReferenceCounted(false);
                    game.lowLatencyWifiLock.acquire();
                }
            }
        } catch (SecurityException e) {
            // Some Samsung Galaxy S10+/S10e devices throw a SecurityException from
            // WifiLock.acquire() even though we have android.permission.WAKE_LOCK in our manifest.
            e.printStackTrace();
        }

        game.appName = game.getIntent().getStringExtra(Game.EXTRA_APP_NAME);
        game.pcName = game.getIntent().getStringExtra(Game.EXTRA_PC_NAME);

        String host = game.getIntent().getStringExtra(Game.EXTRA_HOST);
        int port = game.getIntent().getIntExtra(Game.EXTRA_PORT, NvHTTP.DEFAULT_HTTP_PORT);
        int httpsPort = game.getIntent().getIntExtra(Game.EXTRA_HTTPS_PORT, 0); // 0 is treated as unknown
        int appId = game.getIntent().getIntExtra(Game.EXTRA_APP_ID, StreamConfiguration.INVALID_APP_ID);
        String uniqueId = game.getIntent().getStringExtra(Game.EXTRA_UNIQUEID);
        boolean appSupportsHdr = game.getIntent().getBooleanExtra(Game.EXTRA_APP_HDR, false);
        byte[] derCertData = game.getIntent().getByteArrayExtra(Game.EXTRA_SERVER_CERT);

        game.app = new NvApp(game.appName != null ? game.appName : "app", appId, appSupportsHdr);
        game.app.setAppUuid(game.getIntent().getStringExtra(Game.EXTRA_APP_UUID));
        game.app.setMonitorResume(game.getIntent().getBooleanExtra(Game.EXTRA_MONITOR_RESUME, false));
        game.app.setSmallStreamInputOnly(game.getIntent().getBooleanExtra(Game.EXTRA_SMALL_STREAM_INPUT_ONLY, false));
        // Until the host answers, its last known capabilities decide whether this is a control tile.
        HostCapabilities cachedCaps = HostCapabilityCache.load(game, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID));
        game.app.setHostCapabilities(cachedCaps);
        boolean mediaStream = game.app.getRole() != NvApp.Role.INPUT_ONLY && !game.app.isControlAction();
        game.videoOutput.startInWindowShape(cachedCaps, mediaStream);

        X509Certificate serverCert = null;
        try {
            if (derCertData != null) {
                serverCert = (X509Certificate) CertificateFactory.getInstance("X.509")
                        .generateCertificate(new ByteArrayInputStream(derCertData));
            }
        } catch (CertificateException e) {
            e.printStackTrace();
        }

        if (appId == StreamConfiguration.INVALID_APP_ID && game.app.getAppUuid().isEmpty()) {
            streamFailed = true;
            game.finish();
            return;
        }

        // Initialize the MediaCodec helper before creating the decoder
        GlPreferences glPrefs = GlPreferences.readPreferences(game);
        MediaCodecHelper.initialize(game, glPrefs.glRenderer);

        // Check if the user has enabled HDR
        boolean willStreamHdr = false;
        if (mediaStream && game.prefConfig.enableHdr) {
            // HLG stands in for HDR10 unless the Vulkan renderer draws the picture on a PQ-only surface.
            HdrPolicy.Refusal refusal = HdrPolicy.displayRefusal(game.videoOutput.getVideoDisplay(), true,
                    cachedCaps == null ? 0 : cachedCaps.hdrFormats(),
                    MediaCodecDecoderRenderer.vulkanSurface(game.prefConfig));
            willStreamHdr = refusal == HdrPolicy.Refusal.NONE;
            if (!willStreamHdr) game.overlays.refuseHdr(refusal);
        }

        // Check if the user has enabled performance stats overlay
        if (mediaStream && game.prefConfig.enablePerfOverlay) {
            game.performanceOverlayScroll.setVisibility(View.VISIBLE);
        }

        game.decoderRenderer = new MediaCodecDecoderRenderer(
                game,
                game.prefConfig,
                new CrashListener() {
                    @Override
                    public void notifyCrash(Exception e) {
                        // A stream that is ending, or a TV that just lost its display, is not a decoder
                        // crash: counting it would turn reference frame invalidation off next time.
                        CrashAccounting.Verdict verdict = CrashAccounting.classify(GameConnection.this.streamEnding(),
                                GameConnection.this.isAndroidTv(), game.videoDisplayOff);
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
                        synchronized (game.tombstonePrefs) {
                            if (game.reportedCrash) {
                                return;
                            }
                            game.reportedCrash = true;
                            CrashAccounting.recordGiveUp(game.tombstonePrefs, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID));
                        }
                    }
                },
                game.tombstonePrefs.getInt("CrashCount", 0),
                connMgr.isActiveNetworkMetered(),
                willStreamHdr,
                glPrefs.glRenderer,
                game, game.videoOutput.getVideoDisplay());
        game.decoderRenderer.setPackedFailureListener(game.streamRecovery::onPackedChromaFailure);
        game.decoderRenderer.setStreamSizeListener(new MediaCodecDecoderRenderer.StreamSizeListener() {
            @Override
            public void onStreamSizeChanged(int width, int height) {
                game.runOnUiThread(() -> game.videoOutput.applyStreamSize(width, height));
            }

            @Override
            public void onDecodedSize(int decodedWidth, int decodedHeight, int shownWidth, int shownHeight) {
                game.runOnUiThread(() -> {
                    if (!game.isFinishing() && !game.isDestroyed()) {
                        game.streamView.setDecodedPadding(decodedWidth, decodedHeight, shownWidth, shownHeight);
                    }
                });
            }
        });

        // Don't stream HDR if the decoder can't support it
        if (willStreamHdr) {
            HdrPolicy.Refusal refusal = HdrPolicy.decoderRefusal(game.decoderRenderer.isHevcMain10Hdr10Supported(),
                    game.decoderRenderer.isAv1Main10Supported(), game.decoderRenderer.isPyroWaveSupported());
            if (refusal != HdrPolicy.Refusal.NONE) {
                willStreamHdr = false;
                game.overlays.refuseHdr(refusal);
            }
        }
        game.streamHdr = willStreamHdr;

        // PyroWave was chosen but this PC showed it has none; say once for the PC what is used instead.
        if (mediaStream && game.prefConfig.pyroWaveUnavailable) {
            game.messages.notice(game.getIntent().getStringExtra(Game.EXTRA_PC_UUID), HostNotices.Notice.PYROWAVE_UNAVAILABLE,
                    game.getString(R.string.host_pyrowave_unavailable,
                            game.getResources().getStringArray(R.array.host_profile_codecs)[game.prefConfig.videoFormat.ordinal()]));
        } else if (game.prefConfig.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE) {
            HostNotices.reset(game, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID), HostNotices.Notice.PYROWAVE_UNAVAILABLE);
        }

        // Display a message to the user if HEVC was forced on but we still didn't find a decoder
        if (mediaStream && game.prefConfig.videoFormat == PreferenceConfiguration.FormatOption.FORCE_HEVC && !game.decoderRenderer.isHevcSupported()) {
            game.overlays.hostNotice(HostNotices.Notice.CODEC_UNAVAILABLE, R.string.stream_hevc_decoder_missing);
        }

        // Display a message to the user if AV1 was forced on but we still didn't find a decoder
        if (mediaStream && game.prefConfig.videoFormat == PreferenceConfiguration.FormatOption.FORCE_AV1 && !game.decoderRenderer.isAv1Supported()) {
            game.overlays.hostNotice(HostNotices.Notice.CODEC_UNAVAILABLE, R.string.stream_av1_decoder_missing);
        }

        // 10-bit SDR: the 10-bit formats without HDR. HDR, when granted, wins.
        boolean sdr10Hevc = mediaStream && game.decoderRenderer.isHevcMain10Supported();
        boolean sdr10Av1 = mediaStream && game.decoderRenderer.isAv1Main10SdrSupported();
        Sdr10Policy.Refusal sdr10Refusal = Sdr10Policy.refusal(mediaStream, game.prefConfig.enableSdr10Bit, willStreamHdr,
                sdr10Hevc, sdr10Av1, game.prefConfig.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE,
                game.prefConfig.experimentalPacked444);
        game.streamSdr10 = Sdr10Policy.formats(sdr10Refusal, sdr10Hevc, sdr10Av1) != 0;
        game.decoderRenderer.setSdr10Requested(game.streamSdr10);
        if (Sdr10Policy.worthSaying(sdr10Refusal)) {
            LimeLog.warning("10-bit SDR enabled but not used: " + sdr10Refusal);
            int reason = sdr10Refusal == Sdr10Policy.Refusal.PYROWAVE ? R.string.stream_sdr10_reason_pyrowave :
                    sdr10Refusal == Sdr10Policy.Refusal.PACKED_444 ? R.string.stream_sdr10_reason_packed :
                            R.string.stream_sdr10_reason_decoder;
            game.messages.notice(game.getIntent().getStringExtra(Game.EXTRA_PC_UUID), HostNotices.Notice.SDR10_UNAVAILABLE,
                    game.getString(R.string.stream_sdr10_unavailable, game.getString(reason)));
        }
        // Native 4:4:4 over 10-bit SDR depends on the PC, so the connection decides it (and re-arms the notice).

        int supportedVideoFormats = mediaStream ? game.decoderRenderer.getSupportedVideoFormats(willStreamHdr, game.streamSdr10) : MoonBridge.VIDEO_FORMAT_H264;

        int gamepadMask = ControllerHandler.getAttachedControllerMask(game);
        if (!game.prefConfig.multiController) {
            // Always set gamepad 1 present for when multi-controller is
            // disabled for games that don't properly support detection
            // of gamepads removed and replugged at runtime.
            gamepadMask = 1;
        }
        if (game.prefConfig.onscreenController) {
            // If we're using OSC, always set at least gamepad 1.
            gamepadMask |= 1;
        }
        if (DualScreen.isSecond(game.getIntent())) {
            // The controllers belong to the first screen's stream.
            gamepadMask = 0;
        }

        // Set to the optimal mode for streaming
        float displayRefreshRate = mediaStream ? game.videoOutput.prepareDisplayForRendering() :
                (game.desiredRefreshRate = game.getWindowManager().getDefaultDisplay().getRefreshRate());
        LimeLog.info("Display refresh rate: "+displayRefreshRate);

        // If the user requested frame pacing using a capped FPS, we will need to change our
        // desired FPS setting here in accordance with the active display refresh rate.
        int roundedRefreshRate = Math.round(displayRefreshRate);
        int chosenFrameRate = game.prefConfig.fps;
        if (game.prefConfig.framePacing == PreferenceConfiguration.FRAME_PACING_CAP_FPS) {
            if (game.prefConfig.fps >= roundedRefreshRate) {
                if (game.prefConfig.fps > roundedRefreshRate + 3) {
                    // Use frame drops when rendering above the screen frame rate
                    game.prefConfig.framePacing = PreferenceConfiguration.FRAME_PACING_BALANCED;
                    LimeLog.info("Using drop mode for FPS > Hz");
                } else if (roundedRefreshRate <= 49) {
                    // Let's avoid clearly bogus refresh rates and fall back to legacy rendering
                    game.prefConfig.framePacing = PreferenceConfiguration.FRAME_PACING_BALANCED;
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
        if (mediaStream && game.decoderRenderer.applyOutputPacingFallback() &&
                game.prefConfig.videoFormat != PreferenceConfiguration.FormatOption.FORCE_PYROWAVE) {
            // The session's own setup, not a choice to make: the overlay's status line says it.
            game.messages.note(game.getString(R.string.desktop_pacing_fallback));
        }

        game.streamFps = chosenFrameRate;
        game.streamPanelHz = game.panelMaxRefreshRate;

        // Input-only without the host's own mode: the smallest ordinary stream, which leaves the
        // PC's display mode and its speakers alone.
        boolean smallStream = game.app.isSmallStreamInputOnly();
        StreamConfiguration config = new StreamConfiguration.Builder()
                .setResolution(smallStream ? InputOnlyPolicy.SMALL_WIDTH : game.prefConfig.width,
                        smallStream ? InputOnlyPolicy.SMALL_HEIGHT : game.prefConfig.height)
                .setLaunchRefreshRate(smallStream ? InputOnlyPolicy.SMALL_FPS : game.prefConfig.fps)
                .setLaunchRefreshRateX100(smallStream ? 0 : game.prefConfig.launchRefreshRateX100)
                .setRefreshRate(smallStream ? InputOnlyPolicy.SMALL_FPS : chosenFrameRate)
                .setApp(game.app)
                .setResumeOnly(game.streamRecovery.isResumeOnlyStream())
                .setJoinRunning(game.streamRecovery.isJoinRunningStream())
                .setAllowIdleSessionEnd(!game.getIntent().getBooleanExtra(Game.EXTRA_RECOVERY_MEDIA_FAILURE, false))
                .setNeverReplace(com.limelight.console.OpenPlay.isAutoStart(game.getIntent()))
                .setBitrate(smallStream ? InputOnlyPolicy.SMALL_BITRATE_KBPS : game.prefConfig.bitrate)
                .setProbeFirstBitrate(!smallStream && game.prefConfig.probesFirstBitrate())
                .setAutomaticBitrate(!smallStream && game.prefConfig.automaticBitrate)
                .setBitrateLimit(StreamRateChanges.clientLimitKbps(game.prefConfig.layers == null ? null : game.prefConfig.layers.network,
                        game.prefConfig.mobileBitrateLimit))
                .setFecChoice(game.prefConfig.fecChoice)
                .setRateChange(!smallStream && game.prefConfig.rustEngine)
                .setEnableSops(!smallStream && game.prefConfig.enableSops)
                .enableLocalAudioPlayback(smallStream || game.prefConfig.playHostAudio)
                .setMaxPacketSize(1392)
                .setRemoteConfiguration(StreamConfiguration.STREAM_CFG_AUTO) // NvConnection will perform LAN and VPN detection
                .setSupportedVideoFormats(supportedVideoFormats)
                .setSdr10Bit(game.streamSdr10)
                .setPacked444(game.prefConfig.experimentalPacked444)
                .setAttachedGamepadMask(gamepadMask)
                .setClientRefreshRateX100(Math.round(game.desiredRefreshRate * 100))
                .setVirtualDisplay(!smallStream && game.prefConfig.virtualDisplay, game.prefConfig.virtualDisplayScale)
                .setVirtualDisplayDpi(game.prefConfig.virtualDisplayDpi)
                .setVrr(!smallStream && game.prefConfig.vrr)
                .setClipboardSync(game.prefConfig.clipboardSync && game.prefConfig.rustEngine)
                .setSecondScreen(DualScreen.isSecond(game.getIntent()))
                .setHostDisplay(game.prefConfig.hostDisplay)
                .setAudioConfiguration(smallStream ? MoonBridge.AUDIO_CONFIGURATION_STEREO : game.prefConfig.audioConfiguration)
                .setColorSpace(game.decoderRenderer.getPreferredColorSpace())
                .setColorRange(game.decoderRenderer.getPreferredColorRange())
                .setPersistGamepadsAfterDisconnect(!game.prefConfig.multiController)
                .setDisplayLuminance(smallStream ? null : GameVideoOutput.launchLuminance(game.videoOutput.getVideoDisplay()))
                .build();

        // Initialize the connection
        game.conn = new NvConnection(game.getApplicationContext().createConfigurationContext(game.getResources().getConfiguration()),
                new ComputerDetails.AddressTuple(host, port),
                httpsPort, uniqueId, config,
                PlatformBinding.getCryptoProvider(game), serverCert);
        game.streamRecovery.restoreMicrophoneMute();
        game.conn.setMicrophoneNoticeListener(() -> game.runOnUiThread(game.menuActions::showMicrophoneNotice));
        game.controllerHandler = new ControllerHandler(game, game.conn, game, game.prefConfig);
        if (DualScreen.isSecond(game.getIntent())) {
            // The controllers belong to the first screen's stream. Stopped here, before the launch request
            // lists them and before the connection announces them, the host never hears of a second set.
            game.controllerHandler.stop();
        }
        game.conn.setControllerPreparation(game.controllerHandler::prepareControllersForLaunch);
        game.videoOutput.updateInputPollingRate();
        ((android.hardware.display.DisplayManager) game.getSystemService(Context.DISPLAY_SERVICE))
                .registerDisplayListener(game.inputDisplayListener, game.bitrateHandler);
        game.keyboardTranslator = new KeyboardTranslator();

        InputManager inputManager = (InputManager) game.getSystemService(Context.INPUT_SERVICE);
        inputManager.registerInputDeviceListener(game.keyboardTranslator, null);
        inputManager.registerInputDeviceListener(game.pcKeyboardDeviceListener, null);

        game.touchInput.initializeTouchContexts();

        game.desktopGestures = new com.limelight.ui.DesktopGestures(game, game.streamView,
                game.touchInput::cancelHostTouchInput, game.touchInput::clickRightMouseButton);
        game.trackpadPointer.setViewport(game.desktopGestures.viewport());
        if (game.app.getRole() != NvApp.Role.REMOTE_MONITOR) {
            String profile = com.limelight.binding.input.TouchLayout.profileKey(game.getIntent().getStringExtra(Game.EXTRA_PC_UUID),
                    game.app.getAppUuid(), game.app.getAppId());
            game.gameTouchControls = new com.limelight.ui.GameTouchControls(game, profile,
                    new com.limelight.binding.input.HeldInputState.Sink() {
                        @Override public void key(int key, boolean down) { game.keyboardInput.sendCustomKey(key, down); }
                        @Override public void mouse(int button, boolean down) { game.touchInput.sendCustomMouse(button, down); }
                        @Override public void gamepad(int buttons, short leftX, short leftY, short rightX, short rightY,
                                                      byte leftTrigger, byte rightTrigger) {
                            if (game.controllerHandler != null) game.controllerHandler.reportLayoutGamepadState(buttons,
                                    leftX, leftY, rightX, rightY, leftTrigger, rightTrigger);
                        }
                    }, () -> game.foreground && game.connected && game.grabbedInput && game.streamMenu == null && game.conn.canSendInput(),
                    game.menuActions::showStreamDialog);
            game.gameTouchControls.setStreamTouchListener(game.touchInput::onTouchControlsPassthrough);
            game.gameTouchControls.setGestureCancelListener(game.touchInput::cancelTouchInput);
            ((FrameLayout)game.streamView.getParent()).addView(game.gameTouchControls,
                    new FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT));
        }

        if (game.prefConfig.onscreenController && game.app.getRole() != NvApp.Role.REMOTE_MONITOR) {
            game.showOnscreenController();
        }
        if (game.prefConfig.enableMic && game.app.getRole() != NvApp.Role.REMOTE_MONITOR) {
            game.micButton = new com.limelight.ui.MicButton(game, new com.limelight.ui.MicButton.Host() {
                @Override public boolean offered() { return game.conn != null && game.conn.getMicrophoneStatus().canMute(); }
                @Override public boolean muted() { return game.conn != null && game.conn.isMicrophoneMuted(); }
                @Override public void setMuted(boolean muted) { if (game.conn != null) game.conn.setMicrophoneMuted(muted); }
                @Override public float topRowBottom() {
                    return game.modernController != null && game.prefConfig.onscreenController ? game.modernController.topRowBottom() : Float.NaN;
                }
            });
            ((FrameLayout) game.streamView.getParent()).addView(game.micButton, game.micButton.layoutParams());
            game.touchInput.updateMicButton();
        }
        // On every touch device, so the stream menu's switch can show it; it shows only while its setting is on. A TV
        // gets neither the button nor the switch.
        if (com.limelight.ui.MenuButton.offered(game)) {
            game.menuButton = new com.limelight.ui.MenuButton(game, new com.limelight.ui.MenuButton.Host() {
                @Override public void open() { game.showStreamMenu(); }
                @Override public float topRowBottom() {
                    return game.modernController != null && game.prefConfig.onscreenController ? game.modernController.topRowBottom() : Float.NaN;
                }
            });
            ((FrameLayout) game.streamView.getParent()).addView(game.menuButton, game.menuButton.layoutParams());
            game.touchInput.updateMenuButton();
        }

        if ((game.prefConfig.usbDriver || game.prefConfig.usbDualSense || game.prefConfig.usbSwitchPro) && game.app.getRole() != NvApp.Role.REMOTE_MONITOR) {
            // Start the USB driver
            game.usbDriverServiceBound = game.bindService(new Intent(game, UsbDriverService.class),
                    game.usbDriverServiceConnection, Service.BIND_AUTO_CREATE);
        }

        if (mediaStream && !game.decoderRenderer.isAvcSupported()) {
            if (game.spinner != null) {
                game.spinner.dismiss();
                game.spinner = null;
            }

            // If we can't find an AVC decoder, we can't proceed
            streamFailed = true;
            Dialog.displayDialog(game, game.getResources().getString(R.string.conn_error_title),
                    game.getString(R.string.stream_h264_decoder_missing), true);
            return;
        }

        // The connection will be started when the surface gets created
        if (game.prefConfig.useTextureView) {
            game.renderView.getTextureView().setSurfaceTextureListener(game);
            if (game.renderView.getTextureView().isAvailable()) {
                game.onSurfaceTextureAvailable(game.renderView.getTextureView().getSurfaceTexture(), game.renderView.getWidth(), game.renderView.getHeight());
            }
        } else {
            game.renderView.getHolder().addCallback(game);
        }
    }

    private androidx.appcompat.app.AlertDialog launchConfirmationDialog;

    // The stream's network is lost: the overlay says so until one returns (or the stream ends).
    private boolean streamNetworkLost;

    // Asks a host that sends per-frame timing reports (0x5536) for them, once per connection.
    // Feeds the overlay's host latency line and the latency CSV; the stream description's
    // announcement is the final say, the capability only keeps other hosts from being asked.
    private void requestHostTiming() {
        if (game.conn == null || !game.conn.getHostDetails().caps.has(HostCapabilities.Feature.HOST_TIMING) ||
                !MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_HOST_TIMING)) {
            return;
        }
        MoonBridge.sendHostTimingRequest(true);
    }

    // Asks a host that announces link notices (0x5538) to say when it lowers the video bitrate
    // because of the network, once per connection. The overlay then shows it until the host is
    // back at the requested rate. The C engine does not support it.
    private void requestLinkNotice() {
        game.linkNotice = null;
        if (game.conn == null || !MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_LINK_NOTICE)) {
            return;
        }
        MoonBridge.sendLinkNoticeRequest(true);
    }

    // Clipboard sync (0x5508), when this stream's launch asked a Rubylight PC for it. The C engine has none.
    private void startClipboardSync() {
        if (game.clipboardSync != null) {
            game.clipboardSync.stop();
            game.clipboardSync = null;
        }
        if (game.conn == null || !game.conn.askedForClipboardSync()) return;
        if (!MoonBridge.isRustStream()) {
            LimeLog.warning("Clipboard sync: needs the New streaming engine; this stream has none");
            return;
        }
        game.clipboardSync = com.limelight.utils.ClipboardSync.forStream(game,
                text -> game.messages.alert(text, Toast.LENGTH_LONG));
        game.clipboardSync.connectionStarted(game.hasWindowFocus());
    }

    private volatile Boolean androidTv;

    boolean isAndroidTv() {
        Boolean tv = androidTv;
        if (tv == null) {
            tv = game.getPackageManager().hasSystemFeature(PackageManager.FEATURE_TELEVISION) ||
                    game.getPackageManager().hasSystemFeature(PackageManager.FEATURE_LEANBACK);
            androidTv = tv;
        }
        return tv;
    }

    boolean streamEnding() {
        // hostExitRequested is left out: a failed quit keeps it set for 15 s while still streaming.
        return game.streamTerminated || game.exiting || game.connectionStopThread != null;
    }

    void noteVideoDisplay(Display display, boolean removed) {
        game.videoDisplayOff = removed || !display.isValid() || display.getState() != Display.STATE_ON;
        game.stagedExit.onDisplayChanged(SystemClock.uptimeMillis());
    }

    /** The connecting screen of this stream's game, its cover read off the main thread; cancelling ends the stream. */
    com.limelight.ui.ConnectingScreen showConnecting(String detail) {
        // One connecting window at a time: a reconnect over one still open would leave it behind, over the stream.
        if (game.spinner != null) {
            game.spinner.dismiss();
            game.spinner = null;
        }
        android.content.Intent intent = game.getIntent();
        com.limelight.ui.ConnectingScreen screen = com.limelight.ui.ConnectingScreen.show(game, intent.getStringExtra(Game.EXTRA_APP_NAME),
                intent.getStringExtra(Game.EXTRA_PC_NAME), game.getString(R.string.conn_establishing_title), detail, game::finish);
        if (screen == null) return null;
        readCoverInto(screen);
        return screen;
    }

    /** The cover at the window's size, read off the main thread unless this start already has it. */
    void readCoverInto(com.limelight.ui.ConnectingScreen screen) {
        if (connectingCover != null) {
            screen.showCover(connectingCover);
            return;
        }
        android.content.Intent intent = game.getIntent();
        String uuid = intent.getStringExtra(Game.EXTRA_PC_UUID);
        String art = intent.getStringExtra(Game.EXTRA_APP_ART_KEY);
        if (uuid == null || art == null) return;
        android.util.DisplayMetrics window = game.getResources().getDisplayMetrics();
        int menuWidth = game.getResources().getDimensionPixelSize(R.dimen.stream_menu_cover_width);
        int menuHeight = game.getResources().getDimensionPixelSize(R.dimen.stream_menu_cover_height);
        new Thread(() -> {
            java.io.File file = new com.limelight.grid.assets.DiskAssetLoader(game).getFile(uuid, art);
            android.graphics.Bitmap cover = file.exists()
                    ? com.limelight.ui.ConnectingScreen.readCover(file.getPath(), window.widthPixels, window.heightPixels) : null;
            if (cover == null) return;
            if (menuCover == null) menuCover = menuCover(cover, menuWidth, menuHeight);
            // A start that connected meanwhile keeps only the menu's copy.
            if (!game.connected) connectingCover = cover;
            screen.showCover(cover);
        }, "Connecting cover").start();
    }

    /** The stream menu's copy: the full cover would be held for the whole stream. */
    private static android.graphics.Bitmap menuCover(android.graphics.Bitmap cover, int width, int height) {
        float scale = Math.max((float) width / cover.getWidth(), (float) height / cover.getHeight());
        if (scale >= 1f) return cover;
        try {
            return android.graphics.Bitmap.createScaledBitmap(cover, Math.max(1, Math.round(cover.getWidth() * scale)),
                    Math.max(1, Math.round(cover.getHeight() * scale)), true);
        } catch (OutOfMemoryError e) {
            com.limelight.LimeLog.warning("No memory for the stream menu's cover: " + e);
            return null;
        }
    }

    /** The cover the stream menu and a failure show: the connecting one while it is held, else the menu's copy. */
    android.graphics.Bitmap connectingCover() {
        android.graphics.Bitmap full = connectingCover;
        return full != null ? full : menuCover;
    }

    /** The stream runs: the connecting window's cover is let go, and only the menu's small copy stays. */
    void dropConnectingCover() {
        connectingCover = null;
    }

    private volatile android.graphics.Bitmap connectingCover;
    private volatile android.graphics.Bitmap menuCover;

    void stageStarting(final String stage) {
        game.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (game.spinner != null) {
                    // The word follows the protocol's stage; the failure message names a stage that fails.
                    game.spinner.setStage(game.getResources().getString(R.string.conn_starting));
                }
            }
        });
    }

    private HostLossWatch hostLossWatch;

    // Readies the check after a host's goodbye: the address the stream connected to.
    private void startHostLossWatch() {
        if (hostLossWatch != null) hostLossWatch.stop();
        hostLossWatch = null;
        if (game.conn == null) return;
        if (!MoonBridge.hostLossAvailable()) {
            LimeLog.warning("A PC that shuts down is told only by the recovery running out: libnightjar_core.so did not load");
            return;
        }
        java.net.InetAddress host = HostLossWatch.parse(MoonBridge.getHostAddress());
        if (host == null) {
            LimeLog.warning("A PC that shuts down is told only by the recovery running out: the stream's address is unknown");
            return;
        }
        android.net.ConnectivityManager connectivity =
                (android.net.ConnectivityManager) game.getSystemService(android.content.Context.CONNECTIVITY_SERVICE);
        hostLossWatch = new HostLossWatch(new HostLossWatch.Live(connectivity), host, game.conn.hostHttpsPort());
    }

    void stopConnection() {
        if (hostLossWatch != null) hostLossWatch.stop();
        if (openPlaySettled != null) game.bitrateHandler.removeCallbacks(openPlaySettled);
        game.adaptation.stopFoundationAbr();
        game.touchInput.releaseDesktopInput();
        if (game.trackpadPointer != null) game.trackpadPointer.stopMomentum();
        game.streamRecovery.rememberMicrophoneMute();
        game.streamRecovery.cancelAutomaticRecovery();
        if (game.gameTouchControls != null) game.gameTouchControls.releaseInput();
        game.overlays.stopHardwareMetricsPolling();
        game.getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        if (game.lowLatencyWifiLock != null && game.lowLatencyWifiLock.isHeld()) {
            game.lowLatencyWifiLock.release();
        }
        if (game.highPerfWifiLock != null && game.highPerfWifiLock.isHeld()) {
            game.highPerfWifiLock.release();
        }
        game.streamMenuAction = null;
        game.windowFocusActions.clear();
        game.getWindow().getDecorView().removeCallbacks(game.runWindowFocusActions);
        game.bitrateHandler.removeCallbacks(game.updateBitrate);
        game.bitrateHandler.removeCallbacks(game.updateRateChangeBitrate);
        game.bitrateHandler.removeCallbacks(game.expireRateChanges);
        game.rateChanges = null;
        game.bitrateHandler.removeCallbacks(game.checkReconfigure);
        game.bitrateHandler.removeCallbacks(game.updateQuality);
        game.adaptiveQuality = null;
        game.pendingQualityTarget = null;
        game.adaptiveBitrate = null;
        game.qualityAutoBitrate = false;
        game.bitrateHandler.removeCallbacks(game.adaptation.sampleStreamHealth);
        game.bitrateHandler.removeCallbacks(game.videoOutput.pollContentRateVote);
        game.videoOutput.stopRefreshSwitchPolling();
        if (game.wifiDiagnostics != null) game.wifiDiagnostics.stop();
        game.pyroWaveBitrate = null;
        game.displayCapsReporter = null;
        game.touchInput.stopHostCursorShapes();
        if (game.clipboardSync != null) {
            game.clipboardSync.stop();
            game.clipboardSync = null;
        }
        // Before the connection stops: detaching tells the host.
        if (game.usbForwarding != null) {
            game.usbForwarding.stop();
            game.usbForwarding = null;
        }
        if (game.usbDriverBinder != null) {
            game.usbDriverBinder.setListener(null);
            game.usbDriverBinder.setStateListener(null);
            game.usbDriverBinder.stop();
            game.usbDriverBinder = null;
        }
        if (game.connecting || game.connected) {
            game.connecting = game.connected = false;
            game.videoOutput.updatePipAutoEnter();

            if (game.controllerHandler != null) game.controllerHandler.stop();

            // USB teardown can report a final host mute before releasing its route.
            // Keep that mute after the native connection clears controller state.
            if (game.conn.isMicrophoneMutedForReconnect()) game.getIntent().putExtra(Game.EXTRA_MICROPHONE_MUTED, true);

            // Update GameManager state to indicate we're no longer in game
            UiHelper.notifyStreamEnded(game);

            // Stop may take a few hundred ms to do some network I/O to tell
            // the server we're going away and clean up. Let it run in a separate
            // thread to keep things smooth for the UI. Inside moonlight-common,
            // we prevent another thread from starting a connection before and
            // during the process of stopping this one.
            Thread stopping = new Thread(game.conn::stop, "Stop stream");
            game.connectionStopThread = stopping;
            stopping.start();
            LimeLog.info("Stream stopping");
            // Nothing waits for it on the UI thread. A stop stuck for good (a decoder hung inside
            // MediaCodec) keeps the next stream from starting, so it says where it is stuck.
            new android.os.Handler(android.os.Looper.getMainLooper()).postDelayed(() -> {
                String stuck = stuckStopReport(stopping);
                if (stuck != null) LimeLog.severe(stuck);
            }, STOP_REPORT_MS);
        }
    }

    // How long a stream's stop may run before it is reported as stuck.
    static final long STOP_REPORT_MS = 10_000;

    /** Where a stop still running is stuck, or null once it has ended. */
    static String stuckStopReport(Thread stopping) {
        if (!stopping.isAlive()) return null;
        StringBuilder report = new StringBuilder("Stream stop still running after " + STOP_REPORT_MS / 1000 + " s:");
        for (StackTraceElement frame : stopping.getStackTrace()) report.append("\n  at ").append(frame);
        return report.toString();
    }

    /** The device a resume found the stream on, read on the connection's thread before the stage fails. */
    private volatile String movedAwayTo;

    void streamMoved(String device) {
        LimeLog.info("The resume stopped: the stream moved to " + device);
        movedAwayTo = device;
    }

    void stageFailed(final String stage, final int portFlags, final int errorCode) {
        LimeLog.info("Stream failed");
        streamFailed = true;
        // Perform a connection test if the failure could be due to a blocked port
        // This does network I/O, so don't do it on the main thread.
        // Experimental full chroma refused by the PC (or no mode in common): not a network failure.
        final boolean packedRefused = errorCode == MoonBridge.CLIENT_ERROR_PACKED_444_REFUSED;
        final boolean transientRecoveryFailure = !packedRefused && game.streamRecovery.isRecoveryAttempt() &&
                StreamRecoveryPolicy.transientStartup(stage, portFlags, errorCode);
        final int portTestResult = portFlags == 0 || transientRecoveryFailure || packedRefused ?
                MoonBridge.ML_TEST_RESULT_INCONCLUSIVE :
                MoonBridge.testClientConnectivity(ServerHelper.CONNECTION_TEST_SERVER, 443, portFlags);

        game.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (game.isFinishing() || game.isDestroyed()) {
                    return;
                }
                final String movedTo = movedAwayTo;
                if (movedTo != null) {
                    if (game.spinner != null) {
                        game.spinner.dismiss();
                        game.spinner = null;
                    }
                    game.displayedFailureDialog = true;
                    stopConnection();
                    showMovedPrompt(movedTo, game.conn != null ? game.conn.nativeApiClient() : null);
                    return;
                }
                // Once per session: the replacement starts without full chroma. A refusal in a session
                // already without it (never expected: it does not ask) shows the error as usual.
                if (packedRefused && game.streamRecovery.reconnectWithoutPackedChroma("Full chroma refused at stream start (" +
                        Game.terminationDescription(errorCode) + (game.connectionErrorMessage != null ?
                        "): " + game.connectionErrorMessage : ")"), PackedFallback.refusedToast())) return;
                if (transientRecoveryFailure && game.streamRecovery.tryAutomaticRecovery()) return;
                if (game.spinner != null) {
                    game.spinner.dismiss();
                    game.spinner = null;
                }

                if (!game.displayedFailureDialog && !game.foreground) {
                    game.displayedFailureDialog = true;
                    stopConnection();
                    game.finish();
                    return;
                }
                if (!game.displayedFailureDialog && game.foreground && !game.isFinishing()) {
                    game.displayedFailureDialog = true;
                    LimeLog.severe(stage + " failed: " + errorCode);

                    // If video initialization failed and the surface is still valid, display extra information for the user
                    if (stage.contains("video") && game.videoSurface != null && game.videoSurface.isValid()) {
                        game.messages.alert(game.getResources().getText(R.string.video_decoder_init_failed), Toast.LENGTH_LONG);
                    }

                    // A decoder that rejected the stream is this device's failure, not the host's, and
                    // no port or host advice applies to it.
                    DecoderSetupFailure decoderFailure = game.decoderRenderer == null ? null : game.decoderRenderer.getSetupFailure();
                    boolean decoderFailed = Game.isDecoderStageFailure(decoderFailure, stage);
                    String dialogText = Game.startupFailureText(game, decoderFailure, stage, game.connectionErrorMessage, errorCode);

                    if (portFlags != 0 && !decoderFailed) {
                        dialogText += "\n\n" + game.getResources().getString(R.string.check_ports_msg) + "\n" +
                                MoonBridge.stringifyPortFlags(portFlags, "\n");
                    }

                    if (portTestResult != MoonBridge.ML_TEST_RESULT_INCONCLUSIVE && portTestResult != 0)  {
                        dialogText += "\n\n" + game.getResources().getString(R.string.nettest_text_blocked);
                    }

                    if (game.app.isSmallStreamInputOnly()) {
                        dialogText += "\n\n" + game.getString(R.string.input_only_small_stream_refused);
                    }

                    stopConnection();
                    game.streamRecovery.showReconnectDialog(R.string.conn_error_title, dialogText);
                }
            }
        });
    }

    void connectionTerminated(final int reportedErrorCode) {
        LimeLog.info("Stream disconnected");
        game.streamTerminated = true;
        final boolean exitRequested = game.hostExitRequested;
        streamFailed |= !exitRequested;
        if (reportedErrorCode == MoonBridge.ML_ERROR_GRACEFUL_TERMINATION) {
            LimeLog.info("Connection terminated: the PC ended the stream");
        } else if (exitRequested) {
            LimeLog.info("Connection terminated after this device ended the session: " +
                    Game.terminationDescription(reportedErrorCode));
        } else {
            LimeLog.severe("Connection terminated: " + Game.terminationDescription(reportedErrorCode));
        }
        // After this session's host said goodbye and closed its server (a PC shutting down, a host
        // that stopped): ends here with nothing to ask it or retry. Checked first, because the other
        // checks would wait on a PC that is gone.
        final HostLossWatch hostLoss = hostLossWatch;
        final boolean hostGone = !exitRequested && game.connected && hostLoss != null &&
                StreamRecoveryPolicy.transportTermination(reportedErrorCode) &&
                hostLoss.afterGoodbye() == HostLossWatch.GOODBYE_GONE;
        // Some hosts close the transport before their graceful termination packet arrives.
        // Verify an idle host before either retrying or showing an error, even with recovery off.
        final boolean endedSession = !hostGone && !exitRequested && game.connected &&
                StreamRecoveryPolicy.transportTermination(reportedErrorCode) &&
                game.conn != null && game.conn.hasEndedSession();
        final int errorCode = hostGone ? MoonBridge.CLIENT_ERROR_HOST_GONE : exitRequested || endedSession ?
                MoonBridge.ML_ERROR_GRACEFUL_TERMINATION : reportedErrorCode;
        if (endedSession) LimeLog.info("Connection terminated: the PC ended the previous session");
        // Another device pressed Continue here: this stream must not come back. A stream that reconnects does not wait
        // for the answer here, on a link that may be dead: its resume stops at the running-game check and asks there.
        final boolean mayHaveMoved = !hostGone && !exitRequested && game.conn != null && (reportedErrorCode == MoonBridge.ML_ERROR_GRACEFUL_TERMINATION
                || StreamRecoveryPolicy.transientTermination(reportedErrorCode));
        final boolean recoveryFollows = game.prefConfig != null && game.prefConfig.autoReconnect
                && StreamRecoveryPolicy.recoversTermination(errorCode, game.streamRecovery.videoStarted());
        final String movedTo = mayHaveMoved ? game.conn.streamMovedTo(recoveryFollows) : null;
        final NativeApiClient moveClient = movedTo != null ? game.conn.nativeApiClient() : null;
        if (movedTo != null) LimeLog.info("Connection terminated: the stream moved to " + movedTo);
        // The PC's own words for a failure it ended the stream on, where it says them.
        final String hostFailure = !exitRequested && StreamExitPolicy.hostEndedStream(errorCode) &&
                game.conn != null ? game.conn.hostFailureReason() : null;
        if (hostFailure != null) LimeLog.severe("The PC reported: " + hostFailure);
        final int portFlags = MoonBridge.getPortFlagsFromTerminationErrorCode(errorCode);
        // A stream that never had video does not reconnect: its error (and the port test) shows at once.
        final boolean transientFailure = ContinueHerePolicy.recovers(
                StreamRecoveryPolicy.recoversTermination(errorCode, game.streamRecovery.videoStarted()), movedTo != null);
        // A renderer failure on this device says nothing about the network
        final int portTestResult = errorCode == MoonBridge.ML_ERROR_GRACEFUL_TERMINATION || hostGone ||
                errorCode == MoonBridge.CLIENT_ERROR_VULKAN_RENDERER || errorCode == MoonBridge.CLIENT_ERROR_DECODER ||
                (transientFailure && game.prefConfig != null && game.prefConfig.autoReconnect) ?
                MoonBridge.ML_TEST_RESULT_INCONCLUSIVE :
                MoonBridge.testClientConnectivity(ServerHelper.CONNECTION_TEST_SERVER,443, portFlags);

        game.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (game.isFinishing() || game.isDestroyed()) {
                    return;
                }
                if (errorCode == MoonBridge.ML_ERROR_GRACEFUL_TERMINATION && game.stoppedStreamResumeIntent != null) {
                    // The PC ended a session that leaving the stream kept for resuming.
                    game.stoppedStreamResumeIntent = null;
                    if (movedTo != null) {
                        game.displayedFailureDialog = true;
                        showMovedPrompt(movedTo, moveClient);
                        return;
                    }
                    game.finish();
                    return;
                }
                if (transientFailure && !StreamRecoveryPolicy.transportTermination(errorCode)) {
                    // Keep explicit media/early failures visible if a later recovery attempt
                    // finds the host idle. Transport startup retries carry this same flag.
                    game.getIntent().putExtra(Game.EXTRA_RECOVERY_MEDIA_FAILURE, true);
                }
                game.recoveryBudgetExhausted = false;
                if (transientFailure && game.streamRecovery.tryAutomaticRecovery()) return;
                final boolean recoveryExhausted = transientFailure && game.recoveryBudgetExhausted;
                // The recovery this end counted on when it skipped the read did not come: ask now, off this thread.
                final NvConnection connection = game.conn;
                if (recoveryFollows && mayHaveMoved && connection != null && connection.offersContinueHere()
                        && !game.displayedFailureDialog) {
                    new Thread(() -> {
                        final String late = connection.streamMovedTo(false);
                        final NativeApiClient lateClient = late != null ? connection.nativeApiClient() : null;
                        if (late != null) LimeLog.info("Connection terminated: the stream moved to " + late);
                        game.runOnUiThread(() -> {
                            if (game.isFinishing() || game.isDestroyed()) return;
                            endTerminatedStream(errorCode, late, lateClient, ContinueHerePolicy.recovers(transientFailure, late != null),
                                    recoveryExhausted && late == null, hostFailure, portFlags, portTestResult);
                        });
                    }, "Stream move").start();
                    return;
                }
                endTerminatedStream(errorCode, movedTo, moveClient, transientFailure, recoveryExhausted, hostFailure, portFlags, portTestResult);
            }
        });
    }

    /** What is left of a stream that ended and does not reconnect: says why, or just goes back. On the UI thread. */
    private void endTerminatedStream(int errorCode, String movedTo, NativeApiClient moveClient, boolean transientFailure,
                                     boolean recoveryExhausted, String hostFailure, int portFlags, int portTestResult) {
        if (!game.foreground && transientFailure) game.streamRecovery.rememberStreamForResume(false);
        // Let the display go to sleep now
        game.getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);

        // Stop processing controller input
        if (game.controllerHandler != null) game.controllerHandler.stop();

        // Ungrab input
        game.touchInput.setInputGrabState(false);

        if (!game.displayedFailureDialog) {
            game.displayedFailureDialog = true;
            stopConnection();

            if (movedTo != null) {
                showMovedPrompt(movedTo, moveClient);
                return;
            }

            // Display the error dialog if it was an unexpected termination.
            // Otherwise, just finish the activity immediately.
            if (errorCode != MoonBridge.ML_ERROR_GRACEFUL_TERMINATION) {
                String message;

                if (portTestResult != MoonBridge.ML_TEST_RESULT_INCONCLUSIVE && portTestResult != 0) {
                    // If we got a blocked result, that supersedes any other error message
                    message = game.getResources().getString(R.string.nettest_text_blocked);
                }
                else {
                    switch (errorCode) {
                        case MoonBridge.CLIENT_ERROR_HOST_GONE:
                            message = game.getResources().getString(R.string.lost_connection);
                            break;

                        case MoonBridge.ML_ERROR_NO_VIDEO_TRAFFIC:
                            message = game.getResources().getString(R.string.no_video_received_error);
                            break;

                        case MoonBridge.ML_ERROR_NO_VIDEO_FRAME:
                            MediaCodecDecoderRenderer frames = game.decoderRenderer;
                            message = game.getResources().getString(frames != null && frames.isPyroWaveHlgLost() ?
                                    R.string.stream_hdr_pyrowave_hlg_lost : R.string.no_frame_received_error);
                            break;

                        case MoonBridge.ML_ERROR_UNEXPECTED_EARLY_TERMINATION:
                            // The PC closed the stream before its first picture: often the
                            // game simply exited, not protected content.
                            message = game.getResources().getString(R.string.early_stop_error);
                            break;

                        case MoonBridge.ML_ERROR_PROTECTED_CONTENT:
                            message = game.getResources().getString(R.string.early_termination_error);
                            break;

                        case MoonBridge.ML_ERROR_FRAME_CONVERSION:
                            message = game.getResources().getString(R.string.frame_conversion_error);
                            break;

                        case MoonBridge.CLIENT_ERROR_VULKAN_RENDERER:
                            MediaCodecDecoderRenderer renderer = game.decoderRenderer;
                            String failure = renderer != null ? renderer.getVulkanFailure() : null;
                            message = game.getResources().getString(R.string.vulkan_renderer_failed,
                                    failure != null ? failure : game.getResources().getString(R.string.vulkan_failure_unknown));
                            break;

                        case MoonBridge.CLIENT_ERROR_DECODER:
                            MediaCodecDecoderRenderer decoder = game.decoderRenderer;
                            String reason = decoder != null ? decoder.getDecoderFailure() : null;
                            message = game.getResources().getString(StreamExitPolicy.decoderFailureMessage(
                                            decoder != null && decoder.decoderFailedWithVulkan()),
                                    reason != null ? reason : game.getResources().getString(R.string.vulkan_failure_unknown));
                            break;

                        default:
                            message = game.getResources().getString(R.string.conn_terminated_msg) + "\n\n" +
                                    game.getResources().getString(R.string.error_code_prefix) + " " + Game.errorCodeText(errorCode);
                            if (game.prefConfig != null &&
                                    NetworkGrace.mayHaveGivenUp(game.prefConfig.networkGraceSeconds, errorCode)) {
                                message = game.getResources().getString(R.string.network_grace_gave_up,
                                        game.prefConfig.networkGraceSeconds) + "\n\n" + message;
                            }
                            break;
                    }
                }

                if (hostFailure != null) {
                    message += "\n\n" + game.getResources().getString(R.string.host_failure_reason, hostFailure);
                }

                if (portFlags != 0) {
                    message += "\n\n" + game.getResources().getString(R.string.check_ports_msg) + "\n" +
                            MoonBridge.stringifyPortFlags(portFlags, "\n");
                }

                boolean portTestBlocked = portTestResult != MoonBridge.ML_TEST_RESULT_INCONCLUSIVE &&
                        portTestResult != 0;
                boolean graceGaveUp = game.prefConfig != null &&
                        NetworkGrace.mayHaveGivenUp(game.prefConfig.networkGraceSeconds, errorCode);
                // The PC's reason is worth reading: it often says what to change.
                if (game.foreground && (game.stoppedStreamResumeIntent != null || hostFailure != null ||
                        StreamExitPolicy.terminationNeedsAction(errorCode, portTestBlocked,
                                graceGaveUp, recoveryExhausted))) {
                    game.streamRecovery.showReconnectDialog(R.string.conn_terminated_title, message);
                } else if (game.foreground) {
                    // The host went away or the connection was lost: nothing to act on, so
                    // go back to the list and say why where it outlives this screen. Android
                    // 12+ shows two lines of a text toast, so it is the title and the code.
                    Toast.makeText(game.getApplicationContext(), errorCode == MoonBridge.CLIENT_ERROR_HOST_GONE ?
                            game.getString(R.string.lost_connection) :
                            game.getString(R.string.conn_terminated_title) + "\n" +
                                    game.getString(R.string.error_code_prefix) + " " + Game.errorCodeText(errorCode),
                            Toast.LENGTH_LONG).show();
                    game.finish();
                } else if (game.stoppedStreamResumeIntent == null) {
                    game.finish();
                }
            }
            else {
                game.finish();
            }
        }
    }

    void connectionStatusUpdate(final int connectionStatus) {
        game.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (!game.connected || game.isFinishing() || game.isDestroyed()) {
                    return;
                }
                game.poorConnection = connectionStatus == MoonBridge.CONN_STATUS_POOR;
                // The lost-network notice stays until a network returns.
                if (game.prefConfig.disableWarnings || streamNetworkLost) {
                    return;
                }

                if (connectionStatus == MoonBridge.CONN_STATUS_POOR) {
                    if (game.prefConfig.bitrate > 5000) {
                        game.notificationOverlayView.setText(game.getResources().getString(R.string.slow_connection_msg));
                    }
                    else {
                        game.notificationOverlayView.setText(game.getResources().getString(R.string.poor_connection_msg));
                    }

                    game.requestedNotificationOverlayVisibility = View.VISIBLE;
                }
                else if (connectionStatus == MoonBridge.CONN_STATUS_OKAY) {
                    game.requestedNotificationOverlayVisibility = View.GONE;
                }

                if (!game.isHidingOverlays) {
                    game.notificationOverlayView.setVisibility(game.requestedNotificationOverlayVisibility);
                }
            }
        });
    }

    void streamNetworkLost() {
        game.runOnUiThread(() -> {
            if (!game.connected || game.displayedFailureDialog || game.isFinishing() || game.isDestroyed()) return;
            // A frozen picture is never silent: say what happened, even with warnings off.
            // It promises a reconnect only when one will follow (the connection's timeout, or a
            // return it cannot carry on over, recovers it).
            streamNetworkLost = true;
            game.notificationOverlayView.setText(game.getString(game.streamRecovery.recoversAutomatically() &&
                    game.prefConfig.networkGraceSeconds * 1000 <= NetworkReturnRecovery.PEER_TIMEOUT_MS ?
                    R.string.stream_network_lost_reconnecting : R.string.stream_network_lost));
            game.requestedNotificationOverlayVisibility = View.VISIBLE;
            if (!game.isHidingOverlays) game.notificationOverlayView.setVisibility(View.VISIBLE);
        });
    }

    void streamNetworkReturned(final int change, final boolean metered) {
        game.runOnUiThread(() -> {
            // A return the connection carried on over (no route yet) is reported again once
            // the engine's answer changes (the address arrived): only the first clears the notice.
            if (streamNetworkLost) {
                streamNetworkLost = false;
                game.requestedNotificationOverlayVisibility = View.GONE;
                if (!game.isHidingOverlays) game.notificationOverlayView.setVisibility(View.GONE);
            }
            // Roamed, or carrying on (the same address back, or no route yet): the stream goes on,
            // and its own timeout decides if it cannot.
            // Like a drop, only a stream whose video started reconnects (desktop: reconnects_on_its_own).
            if (!game.connected || game.displayedFailureDialog || game.isFinishing() || game.isDestroyed() || game.prefConfig == null ||
                    !game.streamRecovery.videoStarted() || !NetworkReturnRecovery.reconnectsAtOnce(change == MoonBridge.NETWORK_CANNOT_CONTINUE, metered,
                            game.prefConfig.roamingMetered, game.prefConfig.autoReconnect,
                            game.prefConfig.networkGraceSeconds * 1000)) return;
            // The connection cannot carry on over this network and would only time out first.
            LimeLog.info("The network is back on another address the stream did not roam to: reconnecting now");
            if (!game.streamRecovery.tryAutomaticRecovery(false, true)) {
                LimeLog.warning("The network is back, but no automatic reconnection is possible now");
            }
        });
    }

    /** An automatic start's stream clears the guard once it lasted (OpenPlay); removed if it stops first. */
    private Runnable openPlaySettled;

    /** This screen's start or stream went wrong, whichever way the player then leaves it. */
    private boolean streamFailed;

    /** The player left: an automatic start did not fail, unless it already had or this is its reconnect. */
    void playerLeft() {
        if (streamFailed || !com.limelight.console.OpenPlay.isAutoStart(game.getIntent())) return;
        if (game.streamRecovery.isRecoveryAttempt() && !game.connected) return;
        com.limelight.console.OpenPlay.streamEnded(game, 0, true);
    }

    /** The PC runs another game and nobody agreed to closing it: nothing failed, as when the page saw it first. */
    void automaticStartStopped() {
        com.limelight.console.OpenPlay.streamEnded(game, 0, true);
    }

    void connectionStarted() {
        game.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (!game.connecting || game.isFinishing() || game.isDestroyed()) {
                    return;
                }
                if (com.limelight.console.OpenPlay.isAutoStart(game.getIntent())) {
                    if (openPlaySettled != null) game.bitrateHandler.removeCallbacks(openPlaySettled);
                    openPlaySettled = com.limelight.console.OpenPlay.settleAfter(game, game.bitrateHandler);
                }
                dropConnectingCover();
                if (game.spinner != null) {
                    game.spinner.dismiss();
                    game.spinner = null;
                }

                game.connected = true;
                startHostLossWatch();
                game.conn.refreshNativeHostControl();
                game.getIntent().removeExtra(Game.EXTRA_RECOVERY_MEDIA_FAILURE);
                game.bitrateHandler.removeCallbacks(game.adaptation.sampleStreamHealth);
                game.bitrateHandler.postDelayed(game.adaptation.sampleStreamHealth, 1000);
                game.bitrateHandler.removeCallbacks(game.videoOutput.pollContentRateVote);
                if (game.prefConfig.contentRateFrameRate) game.bitrateHandler.postDelayed(game.videoOutput.pollContentRateVote, 1000);
                game.videoOutput.startRefreshSwitchPolling();
                if (game.wifiDiagnostics == null) game.wifiDiagnostics = new com.limelight.utils.WifiDiagnostics(game);
                game.wifiDiagnostics.start();
                game.streamRecovery.onStreamConnected(SystemClock.elapsedRealtime());
                game.recoveryAppId = game.app.getControl() == NvApp.Control.NONE ? game.app.getAppId() : game.conn.getHostDetails().runningGameId;
                game.recoveryAppUuid = game.app.getControl() == NvApp.Control.NONE ? game.app.getAppUuid() : game.conn.getHostDetails().runningGameUuid;
                game.touchInput.updateGameTouchProfile();
                game.controllerHandler.resendControllerArrivals();
                game.overlays.updateHardwareMetricsPolling();
                LimeLog.info("Stream connected");
                game.connecting = false;
                // Normally started a stage earlier (Game.stageComplete); this catches a game not known by then.
                DualScreen.start(game);
                if (!game.streamRecovery.isRecoveryAttempt()) {
                    com.limelight.update.UpdateFlow.notifyForStream(game, game.conn.getHostDetails(),
                            () -> game.foreground && game.connected, text -> game.messages.alert(text, Toast.LENGTH_LONG));
                }
                if (game.app.getRole() != NvApp.Role.INPUT_ONLY && !game.app.isControlAction()) {
                    game.displayCapsReporter = new DisplayCapsReporter(MoonBridge::sendDisplayCaps);
                    game.videoOutput.reportDisplayCaps();
                }
                game.videoOutput.startReconfigureTracking();
                requestHostTiming();
                requestLinkNotice();
                game.keyboardInput.requestTextInput();
                startClipboardSync();
                game.touchInput.updateHostCursorShapes();
                if (!game.conn.canSendInput()) {
                    if (game.gameTouchControls != null) game.gameTouchControls.hideForPip(true);
                    if (game.virtualController != null) {
                        game.virtualController.hide();
                    }
                    if (game.modernController != null) game.modernController.hide();
                    if (game.micButton != null) game.micButton.setSuppressed(true);
                    game.controllerHandler.stop();
                    game.touchInput.setInputGrabState(false);
                }
                if (game.app.isSmallStreamInputOnly() && game.externalPresentation == null) {
                    ((TextView) game.findViewById(R.id.inputOnlyStatus)).setText(game.getString(
                            R.string.input_only_small_stream_status, InputOnlyPolicy.estimatedKbps() / 1000f));
                }
                game.findViewById(R.id.inputOnlyStatus).setVisibility(
                        game.app.getRole() == NvApp.Role.INPUT_ONLY || game.externalPresentation != null ? View.VISIBLE : View.GONE);
                // A link test before the launch may have started the stream lower than the setting.
                game.currentBitrate = game.app.isSmallStreamInputOnly() ? game.prefConfig.bitrate : game.conn.getStartBitrateKbps();
                game.overlays.notePyroWaveFallback();
                game.adaptation.startStreamBitrate();
                game.adaptation.startAdaptiveQuality();
                game.videoOutput.updatePipAutoEnter();

                if (!game.conn.getHostDetails().hasPermission(ComputerDetails.PERMISSION_INPUT)) {
                    game.messages.alert(R.string.stream_input_denied, Toast.LENGTH_LONG);
                }

                // Hide the mouse cursor now after a short delay.
                // Doing it before dismissing the spinner seems to be undone
                // when the spinner gets displayed. On Android Q, even now
                // is too early to capture. We will delay a second to allow
                // the spinner to dismiss before capturing.
                game.bitrateHandler.postDelayed(new Runnable() {
                    @Override
                    public void run() {
                        if (game.foreground && game.connected && game.streamMenu == null && !game.isFinishing()) {
                            game.touchInput.setInputGrabState(true);
                        }
                    }
                }, 500);

                // Keep the display on
                game.getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);

                // Update GameManager state to indicate we're in game
                UiHelper.notifyStreamConnected(game);

                game.videoOutput.hideSystemUi(1000);

                // The first stream says once how to open the stream menu, after the start settles.
                if (!com.limelight.ui.StreamMenuHint.seen(game)) {
                    game.bitrateHandler.removeCallbacks(game.showStreamMenuHint);
                    game.bitrateHandler.postDelayed(game.showStreamMenuHint, 1500);
                }
            }
        });

        // Report this shortcut being used (off the main thread to prevent ANRs)
        ComputerDetails computer = new ComputerDetails();
        computer.name = game.pcName;
        computer.uuid = game.getIntent().getStringExtra(Game.EXTRA_PC_UUID);
        ShortcutHelper shortcutHelper = new ShortcutHelper(game);
        shortcutHelper.reportComputerShortcutUsed(computer);
        if (game.appName != null) {
            // This may be null if launched from the "Resume Session" PC context menu item
            shortcutHelper.reportGameLaunched(computer, game.app);
        }
    }

    void displayMessage(final String message) {
        game.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                game.connectionErrorMessage = message;
                if (game.connected) {
                    game.messages.alert(message, Toast.LENGTH_LONG);
                }
            }
        });
    }

    /** How often an open "Now playing on X" asks the PC whether the move failed. */
    private static final long MOVE_WATCH_MS = 3000;
    /** How many times it asks: past the two minutes the PC keeps its note, even one that never answers. */
    @androidx.annotation.VisibleForTesting
    static final int MOVE_WATCH_READS = 50;

    /**
     * "Now playing on X" with OK and, on a PC this device may ask, Continue here. Under the PIN lock only OK: the stream
     * screen has no way back from the PIN screen, so taking the stream back goes through the library, which asks. Either
     * way it watches the PC while it is open, since a move that fails gives the game back.
     */
    private void showMovedPrompt(String device, NativeApiClient client) {
        NavigationDialogBuilder builder = new NavigationDialogBuilder(game);
        builder.setTitle(NvConnection.movedMessage(game, device))
                .setCancelable(false)
                .setPositiveButton(android.R.string.ok, (dialog, which) -> game.finish());
        if (client != null && !PinGate.streamLocked(game, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID))) {
            builder.setNegativeButton(R.string.continue_here_action, (dialog, which) -> takeStreamBack(client));
        }
        android.app.Dialog prompt = builder.create();
        game.menuActions.showStreamDialog(prompt);
        if (client != null) watchMovedPrompt(prompt, client, MOVE_WATCH_READS);
    }

    /** One read of the PC, off the UI thread, while {@code prompt} is open; a closed prompt ends this. */
    private void watchMovedPrompt(android.app.Dialog prompt, NativeApiClient client, int readsLeft) {
        if (readsLeft <= 0) return;
        game.bitrateHandler.postDelayed(() -> {
            if (!prompt.isShowing() || game.isFinishing() || game.isDestroyed()) return;
            new Thread(() -> {
                StreamMove read = null;
                try {
                    read = client.streamMove();
                } catch (NativeApiException e) {
                    LimeLog.info("Could not ask the PC whether the move failed: " + e.getMessage());
                }
                final StreamMove answer = read;
                game.runOnUiThread(() -> {
                    if (!prompt.isShowing() || game.isFinishing() || game.isDestroyed()) return;
                    if (answer == null || answer.movedTo != null) {
                        watchMovedPrompt(prompt, client, readsLeft - 1);
                    } else if (ContinueHerePolicy.cameBack(false, answer.ownedHere)) {
                        LimeLog.info("The move failed; the game is this device's again");
                        prompt.dismiss();
                        showCameBackPrompt(client);
                    } else if (answer.pressFailed) {
                        LimeLog.info("The move failed; the game stayed with another device");
                        prompt.dismiss();
                        showMoveFailedPrompt(client);
                    }
                    // Otherwise the note ran out with the game elsewhere, or the game ended: what it said stands.
                });
            }, "Moved prompt").start();
        }, MOVE_WATCH_MS);
    }

    /** The move failed: the game is this device's again, for a minute before close-on-disconnect applies. Resuming asks for the PIN where the lock covers this PC. */
    private void showCameBackPrompt(NativeApiClient client) {
        game.menuActions.showStreamDialog(new NavigationDialogBuilder(game)
                .setTitle(R.string.continue_here_came_back)
                .setCancelable(false)
                .setPositiveButton(R.string.applist_menu_resume, (dialog, which) -> askWhoPlaysThenResume(client))
                .setNegativeButton(android.R.string.ok, (dialog, which) -> game.finish())
                .create());
    }

    /** The move failed, and the game is another device's: this device streamed beside it, and may again. */
    private void showMoveFailedPrompt(NativeApiClient client) {
        game.menuActions.showStreamDialog(new NavigationDialogBuilder(game)
                .setTitle(R.string.continue_here_did_not_move)
                .setCancelable(false)
                .setPositiveButton(R.string.continue_here_stream_too, (dialog, which) -> joinOrResume(client))
                .setNegativeButton(android.R.string.ok, (dialog, which) -> game.finish())
                .create());
    }

    /** The answer may be old by now, so this asks who plays as selecting the game does; a PC that does not say gets the resume, which fails if the game is gone. */
    private void askWhoPlaysThenResume(NativeApiClient client) {
        readWhoPlays(client, "Came back read", read -> {
            if (read != null && playsThisGame(read)) {
                showWhoPlaysPrompt(read, client);
            } else if (read != null && ContinueHerePolicy.joinsAfterRead(false, false, read.pressFailed)) {
                joinTheGame(read);
            } else {
                resumeTheGame();
            }
        });
    }

    /** Stream here too on "did not move", asked again for the same reason. A PC that does not say never gets a join: {@code /resume} would join whatever runs. */
    private void joinOrResume(NativeApiClient client) {
        readWhoPlays(client, "Stream too read", read -> {
            if (read != null && ContinueHerePolicy.joinsAfterRead(true, playsThisGame(read), read.pressFailed)) {
                joinTheGame(read);
            } else {
                resumeTheGame();
            }
        });
    }

    /** Who plays, off the UI thread and within the wait selecting a game has. Leaving meanwhile leaves the start to the reconnect prompt on the way back. */
    private void readWhoPlays(NativeApiClient client, String thread, java.util.function.Consumer<StreamMove> answer) {
        new Thread(() -> {
            StreamMove read = ContinueHereGate.readWithin(client::streamMove, ContinueHereGate.SELECT_READ_WAIT_MS);
            game.runOnUiThread(() -> {
                if (game.isFinishing() || game.isDestroyed()) return;
                if (!game.foreground) {
                    game.streamRecovery.leftDuringPress();
                    return;
                }
                answer.accept(read);
            });
        }, thread).start();
    }

    /** Whether another device plays the game this stream showed; a Resume or Running Game tile keeps its own identity in {@code game.app}. */
    private boolean playsThisGame(StreamMove read) {
        NvApp streamed = game.app;
        if (game.recoveryAppId != 0) {
            streamed = new NvApp("app", game.recoveryAppId, false);
            streamed.setAppUuid(game.recoveryAppUuid);
        }
        return read.playingOn != null && streamed != null && ContinueHerePolicy.asksOnSelect(true, true,
                streamed.matchesRunningApp(read.playingAppId, read.playingAppUuid));
    }

    /** "X is playing on Y". Under the PIN lock no Continue here, as in {@link #showMovedPrompt}: that goes through the library. */
    private void showWhoPlaysPrompt(StreamMove move, NativeApiClient client) {
        NavigationDialogBuilder builder = new NavigationDialogBuilder(game);
        builder.setMessage(game.getString(R.string.continue_here_playing_on, move.playingTitle, move.playingOn))
                .setCancelable(false)
                .setNeutralButton(R.string.continue_here_stream_too, (dialog, which) -> joinTheGame(move))
                .setNegativeButton(android.R.string.cancel, (dialog, which) -> game.finish());
        if (!PinGate.streamLocked(game, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID))) {
            builder.setPositiveButton(R.string.continue_here_action, (dialog, which) -> takeStreamBack(client));
        }
        game.menuActions.showStreamDialog(builder.create());
    }

    /** Joins this device's own running game, behind the PIN where the lock covers this PC. A game that ended meanwhile is not started again. */
    private void resumeTheGame() {
        game.streamRecovery.reconnectStream(
                PinGate.needsPinForStream(game, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID)), true);
    }

    /** Streams beside the device that owns the game, behind the PIN likewise. A game that ended meanwhile is not started again, here either. */
    private void joinTheGame(StreamMove read) {
        game.streamRecovery.joinRunningGame(
                PinGate.needsPinForStream(game, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID)),
                read.playingAppId, read.playingAppUuid);
    }

    /** Continue here on the displaced device: moves the stream back, then resumes. A failure says so; the game keeps running. */
    private void takeStreamBack(NativeApiClient client) {
        new Thread(() -> {
            String failure = null;
            try {
                client.continueHere();
            } catch (NativeApiException e) {
                LimeLog.warning("Continue here failed: " + e.getMessage());
                failure = e.getMessage();
            }
            String why = failure;
            game.runOnUiThread(() -> {
                if (game.isFinishing() || game.isDestroyed()) {
                    return;
                }
                if (!game.foreground) {
                    game.streamRecovery.leftDuringPress();
                    return;
                }
                if (why == null) {
                    // The game may exit before the new stream starts: that must not start it again.
                    game.streamRecovery.reconnectStream(false, true);
                    return;
                }
                game.menuActions.showStreamDialog(new NavigationDialogBuilder(game)
                        .setTitle(R.string.continue_here_failed)
                        .setMessage(why)
                        .setCancelable(false)
                        .setPositiveButton(android.R.string.ok, (dialog, which) -> game.finish())
                        .create());
            });
        }, "Continue here").start();
    }

    void launchConfirmationRequired(LaunchConfirmation confirmation) {
        game.runOnUiThread(() -> {
            if (!game.foreground || game.isFinishing() || confirmation.getState(LaunchConfirmation.nowMs()) != LaunchConfirmation.State.WAITING) {
                confirmation.cancel();
                return;
            }
            if (game.spinner != null) {
                game.spinner.dismiss();
                game.spinner = null;
            }
            game.pendingConfirmation = confirmation;
            launchConfirmationDialog = new com.google.android.material.dialog.MaterialAlertDialogBuilder(
                    new androidx.appcompat.view.ContextThemeWrapper(game, R.style.StreamTheme))
                    .setTitle(R.string.launch_confirm_title)
                    .setMessage(launchConfirmationMessage(confirmation))
                    .setPositiveButton(R.string.launch_confirm, (dialog, which) -> {
                        confirmation.confirm(LaunchConfirmation.nowMs());
                        game.spinner = showConnecting(game.getString(R.string.conn_establishing_msg));
                    })
                    .setNegativeButton(android.R.string.cancel, (dialog, which) -> {
                        confirmation.cancel();
                        game.finish();
                    }).create();
            launchConfirmationDialog.setOnCancelListener(dialog -> {
                confirmation.cancel();
                game.finish();
            });
            launchConfirmationDialog.setCanceledOnTouchOutside(false);
            launchConfirmationDialog.show();
        });
    }

    /** Names the running game and this one when the start knew both; otherwise "the running app". */
    private String launchConfirmationMessage(LaunchConfirmation confirmation) {
        if (confirmation.isTerminate()) {
            return game.getString(R.string.launch_confirm_terminate);
        }
        String replaced = game.getIntent().getStringExtra(Game.EXTRA_REPLACED_APP_NAME);
        return replaced != null && game.appName != null
                ? game.getString(R.string.launch_confirm_replace_named, replaced, game.appName)
                : game.getString(R.string.launch_confirm_replace);
    }

    void launchConfirmationFinished() {
        game.runOnUiThread(() -> {
            game.pendingConfirmation = null;
            if (launchConfirmationDialog != null) {
                launchConfirmationDialog.dismiss();
                launchConfirmationDialog = null;
            }
        });
    }

    void launchActionCompleted(String message) {
        game.runOnUiThread(() -> {
            if (!game.isFinishing()) {
                game.messages.alert(message, Toast.LENGTH_LONG);
                game.finish();
            }
        });
    }

    // The connection's own once-per-PC facts: a virtual display, 4K or HDR the PC cannot offer.
    void displayTransientMessage(final String message) {
        game.runOnUiThread(() -> {
            if (game.prefConfig.disableWarnings) game.messages.note(message);
            else game.messages.countedNotice(message);
        });
    }

    void startConnectionOnSurface() {
        if (!game.surfaceCreated || game.isFinishing() || game.isDestroyed()) {
            return;
        }

        if (!game.attemptedConnection) {
            if (game.streamRecovery.isRecoveryAttempt()) {
                long deadlineFrom = game.streamRecovery.isTripResume() ?
                        game.getIntent().getLongExtra(Game.EXTRA_TRIP_RESUME_STARTED, 0) : game.recoveryPolicy.startedAt();
                long remaining = StreamRecoveryPolicy.WINDOW_MS -
                        (SystemClock.elapsedRealtime() - deadlineFrom);
                if (remaining <= 0) {
                    if (game.spinner != null) { game.spinner.dismiss(); game.spinner = null; }
                    game.streamRecovery.showReconnectDialog(R.string.conn_terminated_title, game.getString(R.string.stream_recovery_exhausted));
                    return;
                }
                game.bitrateHandler.postDelayed(game.recoveryTimeout, remaining);
            }
            game.attemptedConnection = true;
            game.connecting = true;
            LimeLog.info("Stream starting");

            // Update GameManager state to indicate we're "loading" while connecting
            UiHelper.notifyStreamConnecting(game);

            game.decoderRenderer.setRenderTarget(game.videoSurface, game.renderView.getWidth(), game.renderView.getHeight());
            Renderers renderers = renderers(game.app.isSmallStreamInputOnly(), game.prefConfig.muteAudio,
                    game.decoderRenderer, game, game.prefConfig.enableAudioFx,
                    game.prefConfig.usbDualSenseHaptics ? game.controllerHandler.headphoneAudioSink() : null);
            game.conn.start(renderers.audio, renderers.video, game);
        }
    }

    /** The audio renderer and decoder a stream starts with. */
    static final class Renderers {
        final AudioRenderer audio;
        final VideoDecoderRenderer video;

        Renderers(AudioRenderer audio, VideoDecoderRenderer video) {
            this.audio = audio;
            this.video = video;
        }
    }

    /** Input-only shows and plays nothing; Mute audio shows the stream and plays nothing. Both
     *  still receive the audio, so the PC streams as it would. */
    static Renderers renderers(boolean inputOnly, boolean muteAudio, VideoDecoderRenderer decoder, Context context,
                               boolean audioFx, AndroidAudioRenderer.HeadphoneSink headphones) {
        if (inputOnly) return new Renderers(new SilentAudioRenderer(), new DiscardingDecoderRenderer());
        if (muteAudio) return new Renderers(new SilentAudioRenderer(), decoder);
        return new Renderers(new AndroidAudioRenderer(context, audioFx, headphones), decoder);
    }

    void configureVideoSurface(Surface surface) {
        game.videoSurface = surface;
        game.appSurfaceFrameRateVote = null;
        game.surfaceCreated = true;
        if (game.streamRecovery.resumeStoppedStreamIfNeeded()) return;
        LimeLog.info("Video surface: " + (game.prefConfig.useTextureView ? "TextureView" : "SurfaceView") +
                ", pacing: " + game.prefConfig.framePacing + ", newest frame: " + game.prefConfig.dropLateFrames);

        if (game.prefConfig.useTextureView) {
            game.videoOutput.logDisplayRefreshPolicy(game.videoOutput.getVideoDisplay());
            return;
        }

        game.videoOutput.voteSurfaceFrameRate(surface);

        // Disable producer throttling on the underlying surface for reduced latency
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.CINNAMON_BUN) {
            surface.setProducerThrottlingEnabled(false);
        }
    }

    void destroyVideoSurface() {
        if (!game.surfaceCreated) {
            return;
        }
        game.surfaceCreated = false;
        game.videoSurface = null;
        game.appSurfaceFrameRateVote = null;
        if (game.attemptedConnection && game.decoderRenderer != null) {
            // Stop rendering synchronously before TextureView releases its surface.
            // Reconnecting an external output still tears down the connection on a worker.
            game.decoderRenderer.prepareForStop();
        }
        if (game.externalPresentation != null && !game.isFinishing() && !game.isDestroyed()) {
            game.videoOutput.returnVideoToDevice();
            return;
        }
        if (game.attemptedConnection) {
            game.streamRecovery.rememberStreamForResume(true);
            game.displayedFailureDialog = true;
            stopConnection();
            // The stopped decoder cannot be reused. Resume the captured game in a fresh
            // activity when foreground returns; leaving spends nothing from the recovery budget.
            if (game.streamRecovery.finishesWhenStopped()) game.finish();
        }
    }
}
