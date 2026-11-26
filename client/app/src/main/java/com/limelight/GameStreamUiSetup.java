/**
 * Android stream view initialization, preferences/HDR/decoder wiring, touch controls,
 * USB driver binding, and connecting-screen artwork helpers extracted from {@link GameConnection}.
 */
package com.limelight;

import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.graphics.Bitmap;
import android.hardware.input.InputManager;
import android.media.AudioManager;
import android.net.ConnectivityManager;
import android.net.wifi.WifiManager;
import android.os.Build;
import android.os.Bundle;
import android.os.SystemClock;
import android.preference.PreferenceManager;
import android.util.DisplayMetrics;
import android.view.InputDevice;
import android.view.View;
import android.view.Window;
import android.view.WindowManager;
import android.widget.FrameLayout;
import android.widget.Toast;

import com.limelight.binding.PlatformBinding;
import com.limelight.binding.input.ControllerHandler;
import com.limelight.binding.input.HeldInputState;
import com.limelight.binding.input.KeyboardTranslator;
import com.limelight.binding.input.TouchLayout;
import com.limelight.binding.input.capture.InputCaptureManager;
import com.limelight.binding.input.driver.UsbDriverService;
import com.limelight.binding.video.CrashAccounting;
import com.limelight.binding.video.CrashListener;
import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.binding.video.HdrPolicy;
import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.MediaCodecHelper;
import com.limelight.binding.video.PackedFallback;
import com.limelight.binding.video.PerformanceOverlay;
import com.limelight.binding.video.Sdr10Policy;
import com.limelight.computers.HostCapabilityCache;
import com.limelight.computers.HostNotices;
import com.limelight.grid.assets.DiskAssetLoader;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.StreamConfiguration;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarPolicyJni;
import com.limelight.preferences.ControllersChoice;
import com.limelight.preferences.GlPreferences;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.preferences.StreamLayers;
import com.limelight.ui.AdaptiveLayout;
import com.limelight.ui.ConnectingScreen;
import com.limelight.ui.DesktopGestures;
import com.limelight.ui.GameTouchControls;
import com.limelight.ui.LegacyKeyboardInsets;
import com.limelight.ui.MenuButton;
import com.limelight.ui.MicButton;
import com.limelight.ui.OverlayPlacement;
import com.limelight.ui.StreamGlass;
import com.limelight.ui.StreamMenuHint;
import com.limelight.utils.Dialog;
import com.limelight.utils.LaunchSource;
import com.limelight.utils.RedactedLog;
import com.limelight.utils.UiHelper;

import java.io.ByteArrayInputStream;
import java.io.File;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;

final class GameStreamUiSetup {
    private GameStreamUiSetup() {}

    /**
     * Preserves an explicit fractional stream rate (e.g. 120.04 -> 12004) in RTSP ANNOUNCE
     * {@code clientRefreshRateX100} when it matches the negotiated integer frame rate, instead of
     * overwriting it with Android's rounded nominal display refresh rate (#119).
     */
    static int clientRefreshRateX100(int launchRefreshRateX100, int chosenFrameRate, float desiredRefreshRate) {
        if (launchRefreshRateX100 > 0 && (launchRefreshRateX100 + 50) / 100 == chosenFrameRate) {
            return launchRefreshRateX100;
        }
        return Math.round(desiredRefreshRate * 100);
    }

    static boolean onCreate(Game game, GameConnection conn, Bundle savedInstanceState) {
        if (savedInstanceState != null) {
            LimeLog.info("Stream restored after process death");
            game.messages.alert(R.string.stream_restore_help, Toast.LENGTH_LONG);
            return false;
        }
        if (!Game.isValidLaunch(
                game.getIntent().getStringExtra(Game.EXTRA_HOST),
                game.getIntent().getIntExtra(Game.EXTRA_PORT, NvHTTP.DEFAULT_HTTP_PORT),
                game.getIntent().getStringExtra(Game.EXTRA_UNIQUEID),
                game.getIntent().getIntExtra(Game.EXTRA_APP_ID, StreamConfiguration.INVALID_APP_ID),
                game.getIntent().getStringExtra(Game.EXTRA_APP_UUID))) {
            return false;
        }

        UiHelper.setLocale(game);
        game.requestWindowFeature(Window.FEATURE_NO_TITLE);
        game.getWindow().addFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN);
        game.getWindow().getDecorView().setSystemUiVisibility(
                View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                        | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                        | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN);
        game.getWindow().addFlags(WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN);
        game.getWindow().getDecorView().setOnSystemUiVisibilityChangeListener(game);
        game.setVolumeControlStream(AudioManager.STREAM_MUSIC);
        game.setContentView(R.layout.activity_game);
        game.spinner = conn.showConnecting(game.getString(R.string.conn_establishing_msg));

        StreamLayers.Network startNetwork = null;
        String seenNetwork = game.getIntent().getStringExtra(Game.EXTRA_STREAM_NETWORK);
        if (seenNetwork != null) {
            try { startNetwork = StreamLayers.Network.valueOf(seenNetwork); }
            catch (IllegalArgumentException ignored) {}
        }
        game.prefConfig = PreferenceConfiguration.readPreferences(
                game, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID),
                HostCapabilityCache.load(game, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID)),
                game.getIntent().getStringExtra(Game.EXTRA_APP_UUID),
                game.getIntent().getIntExtra(Game.EXTRA_APP_ID, StreamConfiguration.INVALID_APP_ID), startNetwork);
        if (DualScreen.isSecond(game.getIntent())) {
            DualScreen.configureSecond(game.prefConfig, game.getWindowManager().getDefaultDisplay());
        }
        game.prefConfig.experimentalPacked444 = PackedFallback.effective(
                game.prefConfig.experimentalPacked444, game.getIntent().getBooleanExtra(Game.EXTRA_PACKED_OFF, false));
        game.getIntent().putExtra(Game.EXTRA_STREAM_NETWORK, game.prefConfig.layers.network.name());
        HdrPolicy.setDebugHlgOnly(BuildConfig.DEBUG, game.prefConfig.debugHdrHlgOnly);
        game.overlays.showStreamLayers(game.prefConfig, startNetwork == null);
        game.streamRecovery.readRecoveryBudget(SystemClock.elapsedRealtime());
        game.streamRecovery.restoreSessionBitrate();
        game.panelMaxRefreshRate = DisplayFrameRatePolicy.maxRefreshRate(
                game.videoOutput.getVideoDisplay(), game.prefConfig.width, game.prefConfig.height);
        if (game.prefConfig.vrr) {
            game.prefConfig.fps = DisplayFrameRatePolicy.streamFrameRate(game.panelMaxRefreshRate);
            game.prefConfig.launchRefreshRateX100 = Math.max(100, Math.min(100000, Math.round(game.panelMaxRefreshRate * 100)));
        }
        RedactedLog.addSecret(game.getIntent().getStringExtra(Game.EXTRA_PC_NAME));
        RedactedLog.addSecret(game.getIntent().getStringExtra(Game.EXTRA_HOST));
        LimeLog.info(LaunchSource.describe(game.getIntent()));
        LimeLog.info("Stream request: " + game.prefConfig.width + "x" + game.prefConfig.height + " at "
                + game.prefConfig.fps + " fps (resolution " + game.prefConfig.resolutionSource()
                + (game.prefConfig.vrr ? ", VRR at the panel's " + game.panelMaxRefreshRate + " Hz" : "") + ")");
        LimeLog.info("PC's own stream values: "
                + (game.prefConfig.pcSettings == null ? "none" : game.prefConfig.pcSettings));
        if (game.prefConfig.layers != null) LimeLog.info("Stream settings layers: " + StreamLayers.describe(game.prefConfig.layers));

        game.nativeResolutionStream = game.prefConfig.streamFollowsScreen(
                PreferenceConfiguration.nativeResolution(game), game.videoOutput.foldPanelSizes());
        game.keepsRequestedShape = game.nativeResolutionStream
                || PreferenceConfiguration.isNativeResolution(game.prefConfig.width, game.prefConfig.height);
        game.fpsFollowsDisplay = DisplayFrameRatePolicy.fpsFollowsDisplay(
                game.prefConfig.vrr, game.prefConfig.fps,
                DisplayFrameRatePolicy.maxRefreshRate(game.videoOutput.getVideoDisplay()));
        game.nativeTouchEnabled = !game.prefConfig.touchscreenTrackpad
                && PreferenceManager.getDefaultSharedPreferences(game).getBoolean("checkbox_native_touch", false);
        game.tombstonePrefs = game.getSharedPreferences("DecoderTombstone", 0);
        game.streamOrientation = game.prefConfig.streamOrientation;
        game.lastWindowOrientation = game.getResources().getConfiguration().orientation;

        game.videoOutput.setPreferredOrientationForCurrentDisplay();
        game.videoOutput.configureStreamWindow();
        game.streamView = game.findViewById(R.id.surfaceView);
        game.videoOutput.initializeVideoOutput();
        game.streamView.setOnGenericMotionListener(game);
        game.streamView.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> {
            if (game.trackpadPointer != null) game.trackpadPointer.syncCursor();
        });
        game.streamView.setOnKeyListener(game);
        game.streamView.setInputCallbacks(game);
        game.adaptiveLayout = AdaptiveLayout.attach(game, game.findViewById(android.R.id.content));
        game.adaptiveLayout.setInsetsObserver(game.keyboardInput::onWindowInsets);
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            game.legacyKeyboard = new LegacyKeyboardInsets(
                    game, game.findViewById(android.R.id.content), game.keyboardInput::onKeyboardInsets);
            game.legacyKeyboard.start();
        }
        game.streamView.addOnLayoutChangeListener((view, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom) -> {
            if (game.connected) game.videoOutput.updatePipAutoEnter();
        });
        game.findViewById(android.R.id.content).addOnLayoutChangeListener(
                (view, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom) -> game.videoOutput.onPaneLaidOut(view));

        View backgroundTouchView = game.findViewById(R.id.backgroundTouchView);
        backgroundTouchView.setOnTouchListener(game);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            int mask = InputDevice.SOURCE_CLASS_BUTTON | InputDevice.SOURCE_CLASS_JOYSTICK
                    | InputDevice.SOURCE_CLASS_POINTER | InputDevice.SOURCE_CLASS_POSITION
                    | InputDevice.SOURCE_CLASS_TRACKBALL;
            game.streamView.requestUnbufferedDispatch(mask);
            backgroundTouchView.requestUnbufferedDispatch(mask);
        }

        game.notificationOverlayView = game.findViewById(R.id.notificationOverlay);
        game.streamMenuHint = game.findViewById(R.id.streamMenuHint);
        game.streamMenuHint.getViewTreeObserver().addOnGlobalLayoutListener(() -> {
            if (game.streamMenuHint.getVisibility() == View.VISIBLE) {
                StreamMenuHint.place(game.streamMenuHint, game.performanceOverlayScroll);
            }
        });
        game.performanceOverlayView = game.findViewById(R.id.performanceOverlay);
        game.performanceOverlayScroll = game.findViewById(R.id.performanceOverlayScroll);
        StreamGlass.dressHud(game.performanceOverlayScroll, game.streamMenuHint, game.notificationOverlayView);
        game.performanceOverlayScroll.setOnTouchListener((view, event) ->
                !game.overlays.isDebugOverlay() && game.touchInput.onOverlayTouch(view, event));
        game.overlayMode = PerformanceOverlay.normalizeMode(PreferenceManager.getDefaultSharedPreferences(game)
                .getString("performance_overlay_mode", PerformanceOverlay.MODE_COMPACT));
        game.overlayCorner = OverlayPlacement.Corner.parse(game.prefConfig.overlayPosition);
        game.overlays.attachOverlayPlacement((android.view.ViewGroup) game.streamView.getParent());
        game.overlays.updatePerformanceOverlay();

        game.inputCaptureProvider = InputCaptureManager.getInputCaptureProvider(game, game);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            game.streamView.setOnCapturedPointerListener((view, motionEvent) ->
                    game.touchInput.handleMotionEvent(view, motionEvent));
        }

        ConnectivityManager connMgr = (ConnectivityManager) game.getSystemService(Context.CONNECTIVITY_SERVICE);
        if (connMgr.isActiveNetworkMetered() && !game.prefConfig.disableWarnings) {
            game.messages.alert(R.string.conn_metered, Toast.LENGTH_LONG);
        }

        WifiManager wifiMgr = (WifiManager) game.getApplicationContext().getSystemService(Context.WIFI_SERVICE);
        try {
            if (wifiMgr != null) {
                game.highPerfWifiLock = wifiMgr.createWifiLock(
                        WifiManager.WIFI_MODE_FULL_HIGH_PERF, BuildConfig.BRAND_NAME + " High Perf Lock");
                game.highPerfWifiLock.setReferenceCounted(false);
                game.highPerfWifiLock.acquire();
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                    game.lowLatencyWifiLock = wifiMgr.createWifiLock(
                            WifiManager.WIFI_MODE_FULL_LOW_LATENCY, BuildConfig.BRAND_NAME + " Low Latency Lock");
                    game.lowLatencyWifiLock.setReferenceCounted(false);
                    game.lowLatencyWifiLock.acquire();
                }
            }
        } catch (SecurityException e) { e.printStackTrace(); }

        game.appName = game.getIntent().getStringExtra(Game.EXTRA_APP_NAME);
        game.pcName = game.getIntent().getStringExtra(Game.EXTRA_PC_NAME);
        String host = game.getIntent().getStringExtra(Game.EXTRA_HOST);
        int port = game.getIntent().getIntExtra(Game.EXTRA_PORT, NvHTTP.DEFAULT_HTTP_PORT);
        int httpsPort = game.getIntent().getIntExtra(Game.EXTRA_HTTPS_PORT, 0);
        int appId = game.getIntent().getIntExtra(Game.EXTRA_APP_ID, StreamConfiguration.INVALID_APP_ID);
        String uniqueId = game.getIntent().getStringExtra(Game.EXTRA_UNIQUEID);
        boolean appSupportsHdr = game.getIntent().getBooleanExtra(Game.EXTRA_APP_HDR, false);
        byte[] derCertData = game.getIntent().getByteArrayExtra(Game.EXTRA_SERVER_CERT);

        game.app = new NvApp(game.appName != null ? game.appName : "app", appId, appSupportsHdr);
        game.app.setAppUuid(game.getIntent().getStringExtra(Game.EXTRA_APP_UUID));
        game.app.setMonitorResume(game.getIntent().getBooleanExtra(Game.EXTRA_MONITOR_RESUME, false));
        game.app.setSmallStreamInputOnly(game.getIntent().getBooleanExtra(Game.EXTRA_SMALL_STREAM_INPUT_ONLY, false));
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
        } catch (CertificateException e) { e.printStackTrace(); }
        if (appId == StreamConfiguration.INVALID_APP_ID && game.app.getAppUuid().isEmpty()) return false;

        GlPreferences glPrefs = GlPreferences.readPreferences(game);
        MediaCodecHelper.initialize(game, glPrefs.glRenderer);

        boolean willStreamHdr = false;
        if (mediaStream && game.prefConfig.enableHdr) {
            HdrPolicy.Refusal refusal = HdrPolicy.displayRefusal(
                    game.videoOutput.getVideoDisplay(), true,
                    cachedCaps == null ? 0 : cachedCaps.hdrFormats(),
                    MediaCodecDecoderRenderer.vulkanSurface(game.prefConfig, false));
            willStreamHdr = refusal == HdrPolicy.Refusal.NONE;
            if (!willStreamHdr) game.overlays.refuseHdr(refusal);
        }
        if (mediaStream && game.prefConfig.enablePerfOverlay) game.performanceOverlayScroll.setVisibility(View.VISIBLE);

        game.decoderRenderer = new MediaCodecDecoderRenderer(
                game, game.prefConfig, new CrashListener() {
                    @Override public void notifyCrash(Exception e) {
                        CrashAccounting.Verdict verdict = CrashAccounting.classify(
                                conn.streamEnding(), conn.isAndroidTv(), game.videoDisplayOff);
                        if (verdict != CrashAccounting.Verdict.COUNT) {
                            LimeLog.warning("Decoder failure not counted as a crash ("
                                    + (verdict == CrashAccounting.Verdict.DISPLAY_LOST ? "display lost" : "stream ending")
                                    + "): " + e);
                            return;
                        }
                        LimeLog.severe("Decoder crash counted: " + e);
                        synchronized (game.tombstonePrefs) {
                            if (game.reportedCrash) return;
                            game.reportedCrash = true;
                            CrashAccounting.recordGiveUp(game.tombstonePrefs, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID));
                        }
                    }
                },
                game.tombstonePrefs.getInt("CrashCount", 0), connMgr.isActiveNetworkMetered(),
                willStreamHdr, glPrefs.glRenderer, game, game.videoOutput.getVideoDisplay());
        game.decoderRenderer.setPackedFailureListener(game.streamRecovery::onPackedChromaFailure);
        if (mediaStream) game.decoderRenderer.setCoveringSize(game.videoOutput.coveringStreamSize());
        game.decoderRenderer.setStreamSizeListener(new MediaCodecDecoderRenderer.StreamSizeListener() {
            @Override public void onStreamSizeChanged(int width, int height) {
                game.runOnUiThread(() -> game.videoOutput.applyStreamSize(width, height));
            }
            @Override public void onDecodedSize(int decodedWidth, int decodedHeight, int shownWidth, int shownHeight) {
                game.runOnUiThread(() -> {
                    if (!game.isFinishing() && !game.isDestroyed()) {
                        game.streamView.setDecodedPadding(decodedWidth, decodedHeight, shownWidth, shownHeight);
                    }
                });
            }
        });

        if (willStreamHdr) {
            HdrPolicy.Refusal refusal = HdrPolicy.decoderRefusal(
                    game.decoderRenderer.isHevcMain10Hdr10Supported(),
                    game.decoderRenderer.isAv1Main10Supported(),
                    game.decoderRenderer.isPyroWaveSupported());
            if (refusal != HdrPolicy.Refusal.NONE) { willStreamHdr = false; game.overlays.refuseHdr(refusal); }
        }
        game.streamHdr = willStreamHdr;

        if (mediaStream && game.prefConfig.pyroWaveUnavailable) {
            game.messages.notice(game.getIntent().getStringExtra(Game.EXTRA_PC_UUID),
                    HostNotices.Notice.PYROWAVE_UNAVAILABLE,
                    game.getString(R.string.host_pyrowave_unavailable,
                            game.getResources().getStringArray(R.array.host_profile_codecs)[game.prefConfig.videoFormat.ordinal()]));
        } else if (mediaStream && game.prefConfig.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE
                && game.decoderRenderer.isPyroWaveSupported()) {
            HostNotices.reset(game, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID), HostNotices.Notice.PYROWAVE_UNAVAILABLE);
        }
        if (mediaStream && game.prefConfig.videoFormat == PreferenceConfiguration.FormatOption.FORCE_HEVC
                && !game.decoderRenderer.isHevcSupported()) {
            game.overlays.hostNotice(HostNotices.Notice.CODEC_UNAVAILABLE, R.string.stream_hevc_decoder_missing);
        }
        if (mediaStream && game.prefConfig.videoFormat == PreferenceConfiguration.FormatOption.FORCE_AV1
                && !game.decoderRenderer.isAv1Supported()) {
            game.overlays.hostNotice(HostNotices.Notice.CODEC_UNAVAILABLE, R.string.stream_av1_decoder_missing);
        }

        boolean sdr10Hevc = mediaStream && game.decoderRenderer.isHevcMain10Supported();
        boolean sdr10Av1 = mediaStream && game.decoderRenderer.isAv1Main10SdrSupported();
        Sdr10Policy.Refusal sdr10Refusal = Sdr10Policy.refusal(
                mediaStream, game.prefConfig.enableSdr10Bit, willStreamHdr, sdr10Hevc, sdr10Av1,
                game.prefConfig.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE, false);
        game.streamSdr10 = Sdr10Policy.formats(sdr10Refusal, sdr10Hevc, sdr10Av1) != 0;
        game.decoderRenderer.setSdr10Requested(game.streamSdr10);
        if (Sdr10Policy.worthSaying(sdr10Refusal)) {
            LimeLog.warning("10-bit SDR enabled but not used: " + sdr10Refusal);
            int reason = sdr10Refusal == Sdr10Policy.Refusal.PYROWAVE ? R.string.stream_sdr10_reason_pyrowave
                    : sdr10Refusal == Sdr10Policy.Refusal.PACKED_444 ? R.string.stream_sdr10_reason_packed
                    : R.string.stream_sdr10_reason_decoder;
            game.messages.notice(game.getIntent().getStringExtra(Game.EXTRA_PC_UUID), HostNotices.Notice.SDR10_UNAVAILABLE,
                    game.getString(R.string.stream_sdr10_unavailable, game.getString(reason)));
        }
        int supportedVideoFormats = mediaStream
                ? game.decoderRenderer.getSupportedVideoFormats(willStreamHdr, game.streamSdr10)
                : MoonBridge.VIDEO_FORMAT_H264;

        int gamepadMask = ControllerHandler.getAttachedControllerMask(game);
        int connectedControllers = Integer.bitCount(gamepadMask & 0xFFFF);
        if (!game.prefConfig.multiController) gamepadMask = 1;
        if (!game.prefConfig.sendControllers) gamepadMask = 0;
        if (game.prefConfig.onscreenController) gamepadMask |= 1;
        boolean secondScreen = DualScreen.isSecond(game.getIntent());
        if (secondScreen) gamepadMask = 0;
        LimeLog.info("Controllers: " + ControllersChoice.key(game.prefConfig.sendControllers, game.prefConfig.multiController)
                + " from main settings, " + connectedControllers + " connected, gamepad mask 0x"
                + Integer.toHexString(gamepadMask) + (secondScreen ? " (second screen)" : ""));
        if (game.controllersOff == null) {
            game.controllersOff = new ControllersChoice.OffNotice(game.prefConfig.sendControllers, secondScreen);
        }
        game.sayControllersOff(connectedControllers);

        float displayRefreshRate = mediaStream ? game.videoOutput.prepareDisplayForRendering()
                : (game.desiredRefreshRate = game.getWindowManager().getDefaultDisplay().getRefreshRate());
        int chosenFrameRate = conn.streamFrameRate(displayRefreshRate);
        if (mediaStream && game.decoderRenderer.applyOutputPacingFallback()
                && game.prefConfig.videoFormat != PreferenceConfiguration.FormatOption.FORCE_PYROWAVE) {
            game.messages.note(game.getString(R.string.desktop_pacing_fallback));
        }
        game.streamFps = chosenFrameRate;
        game.streamPanelHz = game.panelMaxRefreshRate;

        boolean smallStream = game.app.isSmallStreamInputOnly();
        int launchRefreshRateX100 = smallStream ? 0 : game.prefConfig.launchRefreshRateX100;
        int streamRefreshRate = smallStream ? NightjarPolicyJni.INPUT_ONLY_SMALL_FPS : chosenFrameRate;
        StreamConfiguration config = new StreamConfiguration.Builder()
                .setResolution(smallStream ? NightjarPolicyJni.INPUT_ONLY_SMALL_WIDTH : game.prefConfig.width,
                        smallStream ? NightjarPolicyJni.INPUT_ONLY_SMALL_HEIGHT : game.prefConfig.height)
                .setLaunchRefreshRate(smallStream ? NightjarPolicyJni.INPUT_ONLY_SMALL_FPS : game.prefConfig.fps)
                .setLaunchRefreshRateX100(launchRefreshRateX100)
                .setRefreshRate(streamRefreshRate)
                .setApp(game.app)
                .setResumeOnly(game.streamRecovery.isResumeOnlyStream())
                .setResumePacketSize(game.getIntent().getIntExtra(GameRecovery.EXTRA_RECOVERY_PACKET_SIZE, -1),
                        game.getIntent().getLongExtra(GameRecovery.EXTRA_RECOVERY_PACKET_NETWORK, 0))
                .setJoinRunning(game.streamRecovery.isJoinRunningStream())
                .setAllowIdleSessionEnd(!game.getIntent().getBooleanExtra(Game.EXTRA_RECOVERY_MEDIA_FAILURE, false))
                .setNeverReplace(com.limelight.console.OpenPlay.isAutoStart(game.getIntent()))
                .setBitrate(smallStream ? NightjarPolicyJni.INPUT_ONLY_SMALL_BITRATE_KBPS : game.prefConfig.bitrate)
                .setProbeFirstBitrate(!smallStream && game.prefConfig.probesFirstBitrate())
                .setAutomaticBitrate(!smallStream && game.prefConfig.automaticBitrate)
                .setBitrateLimit(StreamRateChanges.clientLimitKbps(
                        game.prefConfig.layers == null ? null : game.prefConfig.layers.network, game.prefConfig.mobileBitrateLimit))
                .setFecChoice(game.prefConfig.fecChoice).setRateChange(!smallStream)
                .setEnableSops(!smallStream && game.prefConfig.enableSops)
                .enableLocalAudioPlayback(smallStream || game.prefConfig.playHostAudio)
                .setMaxPacketSize(1392).setRemoteConfiguration(StreamConfiguration.STREAM_CFG_AUTO)
                .setSupportedVideoFormats(supportedVideoFormats).setSdr10Bit(game.streamSdr10)
                .setPacked444(mediaStream && game.prefConfig.experimentalPacked444)
                .setSharpColor(mediaStream && (game.prefConfig.enableYuv444 || game.prefConfig.experimentalPacked444),
                        game.prefConfig.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE)
                .setAttachedGamepadMask(gamepadMask)
                .setPlayStationGamepadMask(ControllerHandler.getAttachedPlayStationMask(game))
                .setClientRefreshRateX100(clientRefreshRateX100(launchRefreshRateX100, streamRefreshRate, game.desiredRefreshRate))
                .setVirtualDisplay(!smallStream && game.prefConfig.virtualDisplay, game.prefConfig.virtualDisplayScale)
                .setVirtualDisplayDpi(game.prefConfig.virtualDisplayDpi)
                .setImageEnhancements(game.prefConfig.rtxHdr, game.prefConfig.rtxVideo)
                .setVrr(!smallStream && game.prefConfig.vrr).setClipboardSync(game.prefConfig.clipboardSync)
                .setHostDisplay(game.prefConfig.hostDisplay)
                .setAudioConfiguration(smallStream ? MoonBridge.AUDIO_CONFIGURATION_STEREO : game.prefConfig.audioConfiguration)
                .setColorSpace(game.decoderRenderer.getPreferredColorSpace())
                .setColorRange(game.decoderRenderer.getPreferredColorRange())
                .setPersistGamepadsAfterDisconnect(!game.prefConfig.multiController)
                .setDisplayLuminance(smallStream ? null : GameVideoOutput.launchLuminance(game.videoOutput.getVideoDisplay()))
                .build();

        game.conn = new NvConnection(
                game.getApplicationContext().createConfigurationContext(game.getResources().getConfiguration()),
                new ComputerDetails.AddressTuple(host, port), httpsPort, uniqueId, config,
                PlatformBinding.getCryptoProvider(game), serverCert);
        game.streamRecovery.restoreMicrophoneMute();
        game.conn.setMicrophoneNoticeListener(() -> game.runOnUiThread(game.menuActions::showMicrophoneNotice));
        game.controllerHandler = new ControllerHandler(game, game.conn, game, game.prefConfig);
        game.controllerHandler.setControllerAddedListener(() -> game.sayControllersOff(1));
        game.controllerHandler.setPresenceListener(game::setPhysicalControllerConnected);
        game.physicalControllerConnected = game.controllerHandler.hasExternalControllers();
        if (DualScreen.isSecond(game.getIntent())) game.controllerHandler.stop();
        game.conn.setControllerPreparation(game.controllerHandler::prepareControllersForLaunch);
        game.videoOutput.updateInputPollingRate();
        ((android.hardware.display.DisplayManager) game.getSystemService(Context.DISPLAY_SERVICE))
                .registerDisplayListener(game.inputDisplayListener, game.bitrateHandler);
        game.keyboardTranslator = new KeyboardTranslator();
        InputManager inputManager = (InputManager) game.getSystemService(Context.INPUT_SERVICE);
        inputManager.registerInputDeviceListener(game.keyboardTranslator, null);
        inputManager.registerInputDeviceListener(game.pcKeyboardDeviceListener, null);
        game.touchInput.initializeTouchContexts();
        game.desktopGestures = new DesktopGestures(game, game.streamView,
                game.touchInput::cancelHostTouchInput, game.touchInput::clickRightMouseButton);
        game.trackpadPointer.setViewport(game.desktopGestures.viewport());
        if (game.app.getRole() != NvApp.Role.REMOTE_MONITOR) {
            String profile = TouchLayout.profileKey(
                    game.getIntent().getStringExtra(Game.EXTRA_PC_UUID), game.app.getAppUuid(), game.app.getAppId());
            game.gameTouchControls = new GameTouchControls(game, profile, new HeldInputState.Sink() {
                @Override public void key(int key, boolean down) { game.keyboardInput.sendCustomKey(key, down); }
                @Override public void mouse(int button, boolean down) { game.touchInput.sendCustomMouse(button, down); }
                @Override public void gamepad(int buttons, short lx, short ly, short rx, short ry, byte lt, byte rt) {
                    if (game.controllerHandler != null) game.controllerHandler.reportLayoutGamepadState(buttons, lx, ly, rx, ry, lt, rt);
                }
            }, () -> game.foreground && game.connected && game.grabbedInput && game.streamMenu == null && game.conn.canSendInput(),
                    game.menuActions::showStreamDialog);
            game.gameTouchControls.setStreamTouchListener(game.touchInput::onTouchControlsPassthrough);
            game.gameTouchControls.setGestureCancelListener(game.touchInput::cancelTouchInput);
            game.gameTouchControls.setEditingListener(game::onGameButtonsEditing);
            ((FrameLayout) game.streamView.getParent()).addView(game.gameTouchControls,
                    new FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT));
        }
        if (game.prefConfig.onscreenController && game.app.getRole() != NvApp.Role.REMOTE_MONITOR) game.showOnscreenController();
        if (game.prefConfig.enableMic && game.app.getRole() != NvApp.Role.REMOTE_MONITOR) {
            game.micButton = new MicButton(game, new MicButton.Host() {
                @Override public boolean offered() { return game.conn != null && game.conn.getMicrophoneStatus().canMute(); }
                @Override public boolean muted() { return game.conn != null && game.conn.isMicrophoneMuted(); }
                @Override public void setMuted(boolean muted) { if (game.conn != null) game.conn.setMicrophoneMuted(muted); }
                @Override public float topRowBottom() {
                    return game.modernController != null && game.prefConfig.onscreenController
                            ? game.modernController.topRowBottom() : Float.NaN;
                }
            });
            ((FrameLayout) game.streamView.getParent()).addView(game.micButton, game.micButton.layoutParams());
            game.touchInput.updateMicButton();
        }
        if (MenuButton.offered(game)) {
            game.menuButton = new MenuButton(game, new MenuButton.Host() {
                @Override public void open() { game.showStreamMenu(); }
                @Override public float topRowBottom() {
                    return game.modernController != null && game.prefConfig.onscreenController
                            ? game.modernController.topRowBottom() : Float.NaN;
                }
            });
            ((FrameLayout) game.streamView.getParent()).addView(game.menuButton, game.menuButton.layoutParams());
            game.touchInput.updateMenuButton();
        }
        if ((game.prefConfig.usbDriver || game.prefConfig.usbDualSense || game.prefConfig.usbSwitchPro)
                && game.app.getRole() != NvApp.Role.REMOTE_MONITOR) {
            game.usbDriverReleased = false;
            game.usbDriverServiceBound = game.bindService(
                    new Intent(game, UsbDriverService.class), game.usbDriverServiceConnection, Service.BIND_AUTO_CREATE);
        }
        if (mediaStream && !game.decoderRenderer.isAvcSupported()) {
            if (game.spinner != null) { game.spinner.dismiss(); game.spinner = null; }
            conn.streamFailed = true;
            Dialog.displayDialog(game, game.getResources().getString(R.string.conn_error_title),
                    game.getString(R.string.stream_h264_decoder_missing), true);
            return true;
        }
        if (game.prefConfig.useTextureView) {
            game.renderView.getTextureView().setSurfaceTextureListener(game);
            if (game.renderView.getTextureView().isAvailable()) {
                game.onSurfaceTextureAvailable(
                        game.renderView.getTextureView().getSurfaceTexture(),
                        game.renderView.getWidth(), game.renderView.getHeight());
            }
        } else {
            game.renderView.getHolder().addCallback(game);
        }
        return true;
    }

    static void readCoverInto(Game game, GameConnection conn, ConnectingScreen screen) {
        if (conn.connectingCover != null) { screen.showCover(conn.connectingCover); return; }
        Intent intent = game.getIntent();
        String uuid = intent.getStringExtra(Game.EXTRA_PC_UUID);
        String art = intent.getStringExtra(Game.EXTRA_APP_ART_KEY);
        if (uuid == null || art == null) return;
        DisplayMetrics window = game.getResources().getDisplayMetrics();
        int menuWidth = game.getResources().getDimensionPixelSize(R.dimen.stream_menu_cover_width);
        int menuHeight = game.getResources().getDimensionPixelSize(R.dimen.stream_menu_cover_height);
        new Thread(() -> {
            File file = new DiskAssetLoader(game).getFile(uuid, art);
            Bitmap cover = file.exists()
                    ? ConnectingScreen.readCover(file.getPath(), window.widthPixels, window.heightPixels) : null;
            if (cover == null) return;
            if (conn.menuCover == null) conn.menuCover = menuCover(cover, menuWidth, menuHeight);
            if (!game.connected) conn.connectingCover = cover;
            screen.showCover(cover);
        }, "Connecting cover").start();
    }

    private static Bitmap menuCover(Bitmap cover, int width, int height) {
        float scale = Math.max((float) width / cover.getWidth(), (float) height / cover.getHeight());
        if (scale >= 1f) return cover;
        try {
            return Bitmap.createScaledBitmap(
                    cover, Math.max(1, Math.round(cover.getWidth() * scale)),
                    Math.max(1, Math.round(cover.getHeight() * scale)), true);
        } catch (OutOfMemoryError e) {
            LimeLog.warning("No memory for the stream menu's cover: " + e);
            return null;
        }
    }
}
