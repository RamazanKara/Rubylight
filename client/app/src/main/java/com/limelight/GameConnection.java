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
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.LaunchConfirmation;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.GlPreferences;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.LegacyKeyboardInsets;
import com.limelight.ui.StreamView;
import com.limelight.utils.Dialog;
import com.limelight.utils.NetworkGrace;
import com.limelight.utils.LaunchSource;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.ShortcutHelper;
import com.limelight.utils.SpinnerDialog;
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
            game.finish();
            return;
        }
        if (!Game.isValidLaunch(game.getIntent().getStringExtra(Game.EXTRA_HOST),
                game.getIntent().getIntExtra(Game.EXTRA_PORT, NvHTTP.DEFAULT_HTTP_PORT),
                game.getIntent().getStringExtra(Game.EXTRA_UNIQUEID),
                game.getIntent().getIntExtra(Game.EXTRA_APP_ID, StreamConfiguration.INVALID_APP_ID),
                game.getIntent().getStringExtra(Game.EXTRA_APP_UUID))) {
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
        game.spinner = SpinnerDialog.displayDialog(game, game.getResources().getString(R.string.conn_establishing_title),
                game.getResources().getString(R.string.conn_establishing_msg), true);

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
        game.prefConfig.experimentalPacked444 = PackedFallback.effective(game.prefConfig.experimentalPacked444,
                game.getIntent().getBooleanExtra(Game.EXTRA_PACKED_OFF, false));
        game.getIntent().putExtra(Game.EXTRA_STREAM_NETWORK, game.prefConfig.layers.network.name());
        HdrPolicy.setDebugHlgOnly(BuildConfig.DEBUG, game.prefConfig.debugHdrHlgOnly);
        game.overlays.showStreamLayers(game.prefConfig, startNetwork == null);
        game.streamRecovery.readRecoveryBudget(SystemClock.elapsedRealtime());
        game.panelMaxRefreshRate = com.limelight.binding.video.DisplayFrameRatePolicy.maxRefreshRate(
                game.videoOutput.getVideoDisplay());
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
        game.fpsFollowsDisplay = game.prefConfig.vrr ||
                game.prefConfig.fps == com.limelight.binding.video.DisplayFrameRatePolicy.streamFrameRate(game.panelMaxRefreshRate);
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
                .setAllowIdleSessionEnd(!game.getIntent().getBooleanExtra(Game.EXTRA_RECOVERY_MEDIA_FAILURE, false))
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

    void stageStarting(final String stage) {
        game.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (game.spinner != null) {
                    game.spinner.setMessage(game.getResources().getString(R.string.conn_starting) + " " + stage);
                }
            }
        });
    }

    void stopConnection() {
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
            game.connectionStopThread = new Thread(game.conn::stop, "Stop stream");
            game.connectionStopThread.start();
            LimeLog.info("Stream stopping");
        }
    }

    void stageFailed(final String stage, final int portFlags, final int errorCode) {
        LimeLog.info("Stream failed");
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
        if (reportedErrorCode == MoonBridge.ML_ERROR_GRACEFUL_TERMINATION) {
            LimeLog.info("Connection terminated: the PC ended the stream");
        } else if (exitRequested) {
            LimeLog.info("Connection terminated after this device ended the session: " +
                    Game.terminationDescription(reportedErrorCode));
        } else {
            LimeLog.severe("Connection terminated: " + Game.terminationDescription(reportedErrorCode));
        }
        // Some hosts close the transport before their graceful termination packet arrives.
        // Verify an idle host before either retrying or showing an error, even with recovery off.
        final boolean endedSession = !exitRequested && game.connected &&
                StreamRecoveryPolicy.transportTermination(reportedErrorCode) &&
                game.conn != null && game.conn.hasEndedSession();
        final int errorCode = exitRequested || endedSession ?
                MoonBridge.ML_ERROR_GRACEFUL_TERMINATION : reportedErrorCode;
        if (endedSession) LimeLog.info("Connection terminated: the PC ended the previous session");
        final int portFlags = MoonBridge.getPortFlagsFromTerminationErrorCode(errorCode);
        // A stream that never had video does not reconnect: its error (and the port test) shows at once.
        final boolean transientFailure = StreamRecoveryPolicy.recoversTermination(errorCode, game.streamRecovery.videoStarted());
        // A renderer failure on this device says nothing about the network
        final int portTestResult = errorCode == MoonBridge.ML_ERROR_GRACEFUL_TERMINATION ||
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
                                case MoonBridge.ML_ERROR_NO_VIDEO_TRAFFIC:
                                    message = game.getResources().getString(R.string.no_video_received_error);
                                    break;

                                case MoonBridge.ML_ERROR_NO_VIDEO_FRAME:
                                    MediaCodecDecoderRenderer frames = game.decoderRenderer;
                                    message = game.getResources().getString(frames != null && frames.isPyroWaveHlgLost() ?
                                            R.string.stream_hdr_pyrowave_hlg_lost : R.string.no_frame_received_error);
                                    break;

                                case MoonBridge.ML_ERROR_UNEXPECTED_EARLY_TERMINATION:
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

                        if (portFlags != 0) {
                            message += "\n\n" + game.getResources().getString(R.string.check_ports_msg) + "\n" +
                                    MoonBridge.stringifyPortFlags(portFlags, "\n");
                        }

                        boolean portTestBlocked = portTestResult != MoonBridge.ML_TEST_RESULT_INCONCLUSIVE &&
                                portTestResult != 0;
                        boolean graceGaveUp = game.prefConfig != null &&
                                NetworkGrace.mayHaveGivenUp(game.prefConfig.networkGraceSeconds, errorCode);
                        if (game.foreground && (game.stoppedStreamResumeIntent != null ||
                                StreamExitPolicy.terminationNeedsAction(errorCode, portTestBlocked,
                                        graceGaveUp, recoveryExhausted))) {
                            game.streamRecovery.showReconnectDialog(R.string.conn_terminated_title, message);
                        } else if (game.foreground) {
                            // The host went away or the connection was lost: nothing to act on, so
                            // go back to the list and say why where it outlives this screen. Android
                            // 12+ shows two lines of a text toast, so it is the title and the code.
                            Toast.makeText(game.getApplicationContext(),
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
        });
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

    void connectionStarted() {
        game.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (!game.connecting || game.isFinishing() || game.isDestroyed()) {
                    return;
                }
                if (game.spinner != null) {
                    game.spinner.dismiss();
                    game.spinner = null;
                }

                game.connected = true;
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
                        game.spinner = SpinnerDialog.displayDialog(game, game.getString(R.string.conn_establishing_title),
                                game.getString(R.string.conn_establishing_msg), true);
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
            if (game.app.isSmallStreamInputOnly()) {
                game.conn.start(new SilentAudioRenderer(), new DiscardingDecoderRenderer(), game);
            } else {
                game.conn.start(new AndroidAudioRenderer(game, game.prefConfig.enableAudioFx,
                                game.prefConfig.usbDualSenseHaptics ? game.controllerHandler.headphoneAudioSink() : null),
                        game.decoderRenderer, game);
            }
        }
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
