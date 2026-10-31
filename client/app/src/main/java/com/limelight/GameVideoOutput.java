package com.limelight;

import com.limelight.binding.input.touch.TouchMode;
import com.limelight.binding.video.DisplayCapsReporter;
import com.limelight.binding.video.DisplayRefreshDiagnostics;
import com.limelight.binding.video.HdrFormatPolicy;
import com.limelight.binding.video.HdrPolicy;
import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.binding.video.PerformanceOverlay;
import com.limelight.binding.video.Sdr10Policy;
import com.limelight.binding.video.StreamResizePolicy;
import com.limelight.binding.video.UpscalingPolicy;
import com.limelight.computers.HostNotices;
import com.limelight.nvstream.AdaptiveQualityController;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.HostDisplayChoice;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.StreamOrientation;
import com.limelight.ui.VideoScaling;
import com.limelight.utils.HdrBrightness;
import android.annotation.TargetApi;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowCompat;
import com.limelight.ui.NavigationDialogBuilder;
import android.app.PictureInPictureParams;
import android.app.PendingIntent;
import android.app.RemoteAction;
import android.content.IntentFilter;
import android.graphics.drawable.Icon;
import com.limelight.ui.PictureInPicturePolicy;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.content.res.Configuration;
import android.graphics.Point;
import android.graphics.SurfaceTexture;
import android.graphics.Rect;
import android.media.MediaFormat;
import android.os.Build;
import android.os.Handler;
import android.os.SystemClock;
import android.preference.PreferenceManager;
import android.util.Rational;
import android.view.Display;
import android.view.Surface;
import android.view.View;
import android.view.Window;
import android.view.WindowManager;
import android.widget.TextView;
import android.widget.Toast;

/** Where and how the video is shown: the output display and window shape, picture-in-picture, scaling and zoom, orientation, the stream's size and reconfiguration, refresh rate votes and switches, HDR status and the display's capabilities. */
final class GameVideoOutput {
    private final Game game;

    GameVideoOutput(Game game) {
        this.game = game;
    }

    private boolean streamStartedLandscape = true;

    private int launchWidth, launchHeight;
    // The screen's own resolution when the stream was asked for; null until then.
    private String launchPanel;

    private boolean orientationNoticeShown;
    private String displaySignature = "";
    private int lastPaneWidth, lastPaneHeight;

    private Boolean sdr10LastHostHdr;
    private HdrPolicy.Status lastHdrStatus = HdrPolicy.Status.QUIET;

    // Reports the output's HDR capabilities to Rubylight 2.2.0+ hosts. Keep luminance fixed on
    // that output: brightness changes must not reconfigure HDR on older hosts.
    void reportDisplayCaps() {
        if (!game.connected || game.displayCapsReporter == null ||
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
                displayHdr = HdrPolicy.reportsHdrOutput(hdrCaps.getSupportedHdrTypes(), game.decoderRenderer != null ?
                        game.decoderRenderer.getStreamTransfer() : MediaFormat.COLOR_TRANSFER_ST2084);
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
        DisplayCapsReporter.Caps sent = game.displayCapsReporter.update(output, game.streamHdr && displayHdr,
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

    void updateInputPollingRate() {
        Display display = game.getWindowManager().getDefaultDisplay();
        game.conn.setInputPollingRate(Build.VERSION.SDK_INT >= Build.VERSION_CODES.M ?
                display.getMode().getRefreshRate() : display.getRefreshRate());
    }

    private final DisplayRefreshDiagnostics displayRefreshDiagnostics = new DisplayRefreshDiagnostics();

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
            if (!game.connected || game.isFinishing()) return;
            checkRefreshSwitch();
            checkArrRate();
            checkHdrStatus();
            checkVulkanPace();
            game.bitrateHandler.postDelayed(this, 1000);
        }
    };
    final Runnable pollContentRateVote = new Runnable() {
        @Override public void run() {
            if (!game.connected || game.isFinishing()) return;
            voteContentRate();
            game.bitrateHandler.postDelayed(this, 1000);
        }
    };

    private static final String EXTRA_OUTPUT_DISPLAY = "rubylight.outputDisplay";

    Display getVideoDisplay() {
        // The intent describes the next output while a reconnect is in progress. Keep
        // reporting capabilities for the surface that this activity actually renders.
        if (game.renderView != null) {
            return game.externalPresentation == null ? game.getWindowManager().getDefaultDisplay() :
                    game.externalPresentation.getDisplay();
        }
        int requested = game.getIntent().getIntExtra(EXTRA_OUTPUT_DISPLAY, Display.DEFAULT_DISPLAY);
        if (requested != Display.DEFAULT_DISPLAY) {
            for (Display display : ((android.hardware.display.DisplayManager)game.getSystemService(Context.DISPLAY_SERVICE))
                    .getDisplays(android.hardware.display.DisplayManager.DISPLAY_CATEGORY_PRESENTATION)) {
                if (display.getDisplayId() == requested && display.isValid()) return display;
            }
        }
        return game.getWindowManager().getDefaultDisplay();
    }

    android.view.Window getVideoWindow() {
        return game.externalPresentation == null ? game.getWindow() : game.externalPresentation.getWindow();
    }

    void initializeVideoOutput() {
        Display display = getVideoDisplay();
        game.renderView = game.streamView;
        if (display.getDisplayId() != game.getWindowManager().getDefaultDisplay().getDisplayId()) {
            com.limelight.ui.StreamPresentation presentation = null;
            try {
                presentation = new com.limelight.ui.StreamPresentation(game, display, game.prefConfig.useTextureView);
                game.externalPresentation = presentation;
                game.renderView = presentation.videoView();
                presentation.setOnDismissListener(ignored -> {
                    if (!game.isFinishing() && !game.isDestroyed() && !game.outputSwitchPending) returnVideoToDevice();
                });
                presentation.show();
                game.prefConfig.touchscreenTrackpad = true;
                // The phone is the touchpad; a pointer drawn on it would not match the external screen.
                if (game.prefConfig.touchMode != TouchMode.OFF) game.prefConfig.touchMode = TouchMode.TRACKPAD;
                game.nativeTouchEnabled = false;
                TextView hint = game.findViewById(R.id.inputOnlyStatus);
                hint.setText(R.string.desktop_touchpad_help); hint.setVisibility(View.VISIBLE);
                // Device orientation and window changes must not resize the external video.
                if (game.nativeResolutionStream) {
                    Point size = new Point(); display.getRealSize(size);
                    if (size.x > 0 && size.y > 0) { game.prefConfig.width = size.x; game.prefConfig.height = size.y; }
                }
                game.nativeResolutionStream = false;
                return;
            } catch (WindowManager.InvalidDisplayException | SecurityException | IllegalArgumentException e) {
                if (presentation != null) {
                    presentation.setOnDismissListener(null);
                    presentation.dismiss();
                }
                game.externalPresentation = null; game.renderView = game.streamView;
                game.getIntent().removeExtra(EXTRA_OUTPUT_DISPLAY);
                game.messages.alert(R.string.desktop_display_unavailable, Toast.LENGTH_LONG);
            }
        }
        game.getIntent().removeExtra(EXTRA_OUTPUT_DISPLAY);
        // A hot-unplug can happen between reading preferences and creating the
        // presentation. Do not retain the missing monitor's VRR/FPS launch values.
        game.panelMaxRefreshRate = com.limelight.binding.video.DisplayFrameRatePolicy.maxRefreshRate(getVideoDisplay());
        if (game.prefConfig.vrr) {
            game.prefConfig.fps = com.limelight.binding.video.DisplayFrameRatePolicy.streamFrameRate(game.panelMaxRefreshRate);
            game.prefConfig.launchRefreshRateX100 = Math.max(100, Math.min(100000, Math.round(game.panelMaxRefreshRate * 100)));
        }
        game.fpsFollowsDisplay = game.prefConfig.vrr || game.prefConfig.fps ==
                com.limelight.binding.video.DisplayFrameRatePolicy.streamFrameRate(game.panelMaxRefreshRate);
        game.streamView.initializeSurface(game.prefConfig.useTextureView);
    }

    void returnVideoToDevice() {
        if (game.outputSwitchPending || game.isFinishing() || game.isDestroyed()) return;
        game.outputSwitchPending = true;
        game.getIntent().removeExtra(EXTRA_OUTPUT_DISPLAY);
        if (game.foreground && game.conn != null) game.streamRecovery.reconnectStream();
        else {
            game.streamRecovery.rememberStreamForResume(true);
            if (game.stoppedStreamResumeIntent != null) {
                game.stoppedStreamResumeIntent.removeExtra(EXTRA_OUTPUT_DISPLAY);
                game.displayedFailureDialog = true;
                game.stopConnection();
            } else game.finish();
        }
    }

    void toggleDesktopZoom() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N && !game.prefConfig.useTextureView) {
            game.menuActions.showStreamDialog(new NavigationDialogBuilder(game).setMessage(R.string.desktop_zoom_surface_help)
                    .setPositiveButton(android.R.string.ok, null).create());
            return;
        }
        game.desktopGestures.setEnabled(!game.desktopGestures.enabled());
        if (game.desktopGestures.enabled()) game.messages.alert(R.string.desktop_zoom_help, Toast.LENGTH_LONG);
    }

    boolean hasPresentationDisplays() {
        return ((android.hardware.display.DisplayManager) game.getSystemService(Context.DISPLAY_SERVICE))
                .getDisplays(android.hardware.display.DisplayManager.DISPLAY_CATEGORY_PRESENTATION).length > 0;
    }

    // The view changes its scaling at once. An upscaler or PyroWave sets its own output rectangle
    // when the stream starts, so for those the choice waits for the next stream.
    boolean canScaleLive() {
        return game.externalPresentation == null && game.decoderRenderer != null &&
                game.prefConfig.upscalingMode == UpscalingPolicy.Mode.OFF &&
                (game.decoderRenderer.getActiveVideoFormat() & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) == 0;
    }

    void showVideoScalingDialog() {
        if (!canScaleLive()) return;
        game.menuActions.showStreamDialog(new NavigationDialogBuilder(game).setTitle(R.string.video_scaling_title)
                .setSingleChoiceItems(R.array.video_scaling_modes, game.prefConfig.videoScaling, (dialog, which) -> {
                    dialog.dismiss();
                    setVideoScaling(which);
                }).setNegativeButton(android.R.string.cancel, null).create());
    }

    private void setVideoScaling(int mode) {
        if (mode == game.prefConfig.videoScaling || !canScaleLive()) return;
        game.prefConfig.setVideoScaling(mode);
        SharedPreferences.Editor editor = PreferenceManager.getDefaultSharedPreferences(game).edit();
        VideoScaling.save(editor, mode);
        editor.apply();
        applyVideoScaling();
        // Fill and Stretch span the cutout too; the safe area follows.
        configureStreamWindow();
        updatePipAutoEnter();
    }

    /** Sizes the video view for the scaling mode; picture-in-picture always shows the whole stream. */
    void applyVideoScaling() {
        if (game.renderView == null || game.prefConfig == null) return;
        int mode = game.prefConfig.videoScaling;
        if (game.renderView == game.streamView && Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && game.isInPictureInPictureMode()) {
            mode = VideoScaling.forPictureInPicture(mode);
        }
        game.renderView.setVideoScaling(mode, game.prefConfig.videoAlignment, game.prefConfig.width, game.prefConfig.height);
    }

    boolean canEnterPictureInPicture() {
        return Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && game.prefConfig.enablePip && game.externalPresentation == null &&
                game.app.getRole() != NvApp.Role.INPUT_ONLY &&
                game.getPackageManager().hasSystemFeature(PackageManager.FEATURE_PICTURE_IN_PICTURE);
    }

    void enterPictureInPictureNow() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O || !canEnterPictureInPicture() || !game.connected) return;
        try {
            game.enterPictureInPictureMode(getPictureInPictureParams(false));
        } catch (IllegalStateException | IllegalArgumentException e) {
            LimeLog.warning("Picture-in-picture unavailable");
        }
    }

    void showVideoOutputDialog() {
        Display[] displays = ((android.hardware.display.DisplayManager)game.getSystemService(Context.DISPLAY_SERVICE))
                .getDisplays(android.hardware.display.DisplayManager.DISPLAY_CATEGORY_PRESENTATION);
        String[] labels = new String[displays.length + 1]; labels[0] = game.getString(R.string.desktop_output_device);
        for (int i = 0; i < displays.length; i++) labels[i + 1] = displays[i].getName();
        game.menuActions.showStreamDialog(new NavigationDialogBuilder(game).setTitle(R.string.desktop_output)
                .setItems(labels, (dialog, which) -> {
                    int id = which == 0 ? Display.DEFAULT_DISPLAY : displays[which - 1].getDisplayId();
                    if (id == game.getIntent().getIntExtra(EXTRA_OUTPUT_DISPLAY, Display.DEFAULT_DISPLAY)) return;
                    game.streamMenuAction = () -> {
                        game.outputSwitchPending = true;
                        game.getIntent().putExtra(EXTRA_OUTPUT_DISPLAY, id);
                        game.streamRecovery.reconnectStream();
                    };
                }).setNegativeButton(android.R.string.cancel, null).create());
        if (displays.length == 0) game.messages.alert(R.string.desktop_display_unavailable, Toast.LENGTH_LONG);
    }

    void setPreferredOrientationForCurrentDisplay() {
        boolean freeWindow = game.getIntent().getIntExtra(EXTRA_OUTPUT_DISPLAY, Display.DEFAULT_DISPLAY) != Display.DEFAULT_DISPLAY ||
                (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N && game.isInMultiWindowMode());
        boolean largeScreen = game.getResources().getConfiguration().smallestScreenWidthDp >= 600 ||
                game.getPackageManager().hasSystemFeature("android.hardware.type.pc");
        boolean squarish = !freeWindow && !largeScreen &&
                PreferenceConfiguration.isSquarishScreen(game.getWindowManager().getDefaultDisplay());
        PreferenceConfiguration config = game.prefConfig != null ? game.prefConfig : new PreferenceConfiguration();
        game.setRequestedOrientation(StreamOrientation.requested(game.streamOrientation, freeWindow, largeScreen, squarish,
                config.onscreenController, game.nativeResolutionStream, config.width, config.height));
    }

    // A host that draws the stream on its virtual display starts it in the shape the Orientation
    // setting gives the window; any other host keeps the size and the video is scaled into it.
    void startInWindowShape(HostCapabilities caps, boolean mediaStream) {
        HostDisplayChoice display = game.prefConfig.hostDisplay == null || !caps.has(HostCapabilities.Feature.HOST_DISPLAY_MODES) ?
                HostDisplayChoice.HOST_DEFAULT : game.prefConfig.hostDisplay.forRole(game.app.getRole());
        game.hostShapesStream = mediaStream && game.externalPresentation == null && game.app.getRole() == NvApp.Role.STREAM &&
                caps.has(HostCapabilities.Feature.VIRTUAL_DISPLAY) && display.requestsVirtualDisplay(game.prefConfig.virtualDisplay);
        boolean windowPortrait = game.getResources().getConfiguration().orientation == Configuration.ORIENTATION_PORTRAIT;
        int[] size = StreamOrientation.launchSize(game.streamOrientation, game.hostShapesStream, windowPortrait,
                game.prefConfig.width, game.prefConfig.height);
        if (size[0] != game.prefConfig.width || size[1] != game.prefConfig.height) {
            LimeLog.info("Orientation " + StreamOrientation.value(game.streamOrientation) + ": stream request turned to " +
                    size[0] + "x" + size[1]);
            game.prefConfig.width = size[0];
            game.prefConfig.height = size[1];
        }
        launchWidth = game.prefConfig.width;
        launchHeight = game.prefConfig.height;
        launchPanel = PreferenceConfiguration.nativeResolution(game);
    }

    @TargetApi(Build.VERSION_CODES.O)
    PictureInPictureParams getPictureInPictureParams(boolean autoEnter) {
        int[] ratio = PictureInPicturePolicy.aspectRatio(game.prefConfig.width, game.prefConfig.height);
        Rect source = new Rect();
        game.streamView.getGlobalVisibleRect(source);
        PictureInPictureParams.Builder builder =
                new PictureInPictureParams.Builder()
                        .setAspectRatio(new Rational(ratio[0], ratio[1]))
                        .setSourceRectHint(source);

        if (game.pipDisconnectIntent == null) {
            String action = game.getPackageName() + ".PIP_DISCONNECT." + java.util.UUID.randomUUID();
            androidx.core.content.ContextCompat.registerReceiver(game, game.pipReceiver, new IntentFilter(action),
                    androidx.core.content.ContextCompat.RECEIVER_NOT_EXPORTED);
            game.pipDisconnectIntent = PendingIntent.getBroadcast(game, 0,
                    new Intent(action).setPackage(game.getPackageName()), PendingIntent.FLAG_IMMUTABLE);
        }
        RemoteAction disconnect = new RemoteAction(Icon.createWithResource(game, R.drawable.ic_close),
                game.getString(R.string.stream_disconnect), game.getString(R.string.pip_disconnect_description), game.pipDisconnectIntent);
        disconnect.setEnabled(game.connected && game.app.getRole() != NvApp.Role.INPUT_ONLY);
        builder.setActions(java.util.Collections.singletonList(disconnect));

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            builder.setAutoEnterEnabled(autoEnter);
            builder.setSeamlessResizeEnabled(true);
        }

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            if (game.appName != null) {
                builder.setTitle(game.appName);
                if (game.pcName != null) {
                    builder.setSubtitle(game.pcName);
                }
            }
            else if (game.pcName != null) {
                builder.setTitle(game.pcName);
            }
        }

        return builder.build();
    }

    void updatePipAutoEnter() {
        if (game.prefConfig == null || !game.prefConfig.enablePip || game.isFinishing() || game.isDestroyed() ||
                !game.getPackageManager().hasSystemFeature(PackageManager.FEATURE_PICTURE_IN_PICTURE)) {
            return;
        }

        boolean autoEnter = game.externalPresentation == null && PictureInPicturePolicy.shouldAutoEnter(game.connected, game.app.getRole(), game.suppressPipRefCount,
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.N && game.isInMultiWindowMode());

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            try {
                game.setPictureInPictureParams(getPictureInPictureParams(autoEnter));
            } catch (IllegalStateException | IllegalArgumentException e) {
                LimeLog.warning("Picture-in-picture unavailable");
            }
        }
        else {
            game.autoEnterPip = autoEnter;
        }
    }

    private boolean isRefreshRateEqualMatch(float refreshRate) {
        return refreshRate >= game.prefConfig.fps &&
                refreshRate <= game.prefConfig.fps + 3;
    }

    private boolean isRefreshRateGoodMatch(float refreshRate) {
        return refreshRate >= game.prefConfig.fps &&
                Math.round(refreshRate) % game.prefConfig.fps <= 3;
    }

    private boolean shouldIgnoreInsetsForResolution(int width, int height) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            Display display = game.getWindowManager().getDefaultDisplay();
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
        game.getWindowManager().getDefaultDisplay().getRealSize(real);
        if ((width == real.x && height == real.y) || (width == real.y && height == real.x)) {
            return true;
        }
        // A stream in the panel's own shape at another size (a scaled native size, or a PC profile's
        // "match screen" size) is drawn for the whole panel too: keeping the cutout clear would only
        // shrink it and leave a strip. The cutout-safe Native entry is narrower by the cutout and
        // keeps its strip.
        return PreferenceConfiguration.hasPanelShape(game, width, height, PANEL_SHAPE_TOLERANCE);
    }

    // Rounding a scaled panel size to even pixels moves its shape by well under this; leaving out a
    // camera cutout (54 px of a Fold's 2184, 80 px of a phone's 2400) moves it by 2-3%.
    private static final double PANEL_SHAPE_TOLERANCE = 0.005;

    boolean isWindowedStream() {
        return Build.VERSION.SDK_INT >= Build.VERSION_CODES.N && game.isInMultiWindowMode() && !game.windowCoversDisplay();
    }

    /** A free-form or pop-up window maximised to the whole display reports multi-window mode too. */
    boolean windowCoversDisplay() {
        androidx.window.layout.WindowMetricsCalculator calculator =
                androidx.window.layout.WindowMetricsCalculator.getOrCreate();
        return calculator.computeCurrentWindowMetrics(game).getBounds()
                .equals(calculator.computeMaximumWindowMetrics(game).getBounds());
    }

    /** Fullscreen native/fill video uses the complete panel; fixed presets retain a cutout-safe area. */
    boolean usesFullDisplayForVideo() {
        // nativeResolutionStream also includes custom sizes and the explicit cutout-safe
        // Native entry. Only the user's fill/follow choice or an exact panel size spans a notch.
        return game.prefConfig != null && (VideoScaling.coversArea(game.prefConfig.videoScaling) ||
                game.prefConfig.resolutionFollowsScreen || shouldIgnoreInsetsForResolution(game.prefConfig.width, game.prefConfig.height));
    }

    void configureStreamWindow() {
        // Let AdaptiveLayout own the safe area. Combining framework cutout avoidance with
        // root padding cropped the video twice on older Android versions, and DEFAULT is
        // treated as ALWAYS on Android 15+ regardless of the raw layout attribute.
        WindowCompat.setDecorFitsSystemWindows(game.getWindow(), false);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            WindowManager.LayoutParams attributes = game.getWindow().getAttributes();
            attributes.layoutInDisplayCutoutMode = Build.VERSION.SDK_INT >= Build.VERSION_CODES.R ?
                    WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS :
                    WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
            game.getWindow().setAttributes(attributes);
        }
        View root = game.findViewById(android.R.id.content);
        if (root != null) ViewCompat.requestApplyInsets(root);
    }

    private boolean mayReduceRefreshRate() {
        return game.prefConfig.framePacing == PreferenceConfiguration.FRAME_PACING_CAP_FPS ||
                game.prefConfig.framePacing == PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS ||
                game.prefConfig.reduceRefreshRate;
    }

    float prepareDisplayForRendering() {
        Display display = getVideoDisplay();
        WindowManager.LayoutParams windowLayoutParams = getVideoWindow().getAttributes();
        float displayRefreshRate;

        boolean arrSupported = Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA && display.hasArrSupport();
        game.useArr = com.limelight.binding.video.DisplayFrameRatePolicy.useAdaptiveRefreshRate(
                Build.VERSION.SDK_INT, game.prefConfig.vrr, arrSupported);
        logDisplayRefreshPolicy(display, arrSupported);
        if (game.prefConfig.vrr) {
            com.limelight.binding.video.DisplayFrameRatePolicy.ArrState arr =
                    com.limelight.binding.video.DisplayFrameRatePolicy.arrState(Build.VERSION.SDK_INT, true, arrSupported);
            LimeLog.info(game.useArr ? "VRR: ARR (Android automatic policy)" :
                    "VRR: max Hz (ARR unavailable: " + arr + ")");
            // Asked for and not granted: the status line says why, in place of a silent fallback.
            if (arr == com.limelight.binding.video.DisplayFrameRatePolicy.ArrState.ANDROID_TOO_OLD) {
                game.messages.note(game.getString(R.string.stream_vrr_old_android));
            } else if (arr == com.limelight.binding.video.DisplayFrameRatePolicy.ArrState.PANEL_WITHOUT_ARR) {
                game.messages.note(game.getString(R.string.stream_vrr_no_arr));
            }
        }

        if (game.useArr && Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA) {
            // Leave refresh-rate selection and touch boost to Android's automatic ARR policy.
            windowLayoutParams.preferredDisplayModeId = 0;
            windowLayoutParams.preferredRefreshRate = 0;
            getVideoWindow().setAttributes(windowLayoutParams);
            displayRefreshRate = game.panelMaxRefreshRate;
        }
        // On M, we can explicitly set the optimal display mode
        else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            Display.Mode bestMode = display.getMode();
            boolean isNativeResolutionStream = PreferenceConfiguration.isNativeResolution(game.prefConfig.width, game.prefConfig.height);
            boolean refreshRateIsGood = isRefreshRateGoodMatch(bestMode.getRefreshRate());
            boolean refreshRateIsEqual = isRefreshRateEqualMatch(bestMode.getRefreshRate());

            LimeLog.info("Current display mode: "+bestMode.getPhysicalWidth()+"x"+
                    bestMode.getPhysicalHeight()+"x"+bestMode.getRefreshRate());

            for (Display.Mode candidate : display.getSupportedModes()) {
                boolean refreshRateReduced = candidate.getRefreshRate() < bestMode.getRefreshRate();
                boolean resolutionReduced = candidate.getPhysicalWidth() < bestMode.getPhysicalWidth() ||
                        candidate.getPhysicalHeight() < bestMode.getPhysicalHeight();
                boolean resolutionFitsStream = candidate.getPhysicalWidth() >= game.prefConfig.width &&
                        candidate.getPhysicalHeight() >= game.prefConfig.height;

                LimeLog.info("Examining display mode: "+candidate.getPhysicalWidth()+"x"+
                        candidate.getPhysicalHeight()+"x"+candidate.getRefreshRate());

                if (game.prefConfig.vrr) {
                    if (candidate.getRefreshRate() > bestMode.getRefreshRate()) bestMode = candidate;
                    continue;
                }

                if (candidate.getPhysicalWidth() > 4096 && game.prefConfig.width <= 4096) {
                    // Avoid resolutions options above 4K to be safe
                    continue;
                }

                // On non-4K streams, we force the resolution to never change unless it's above
                // 60 FPS, which may require a resolution reduction due to HDMI bandwidth limitations,
                // or it's a native resolution stream.
                if (game.prefConfig.width < 3840 && game.prefConfig.fps <= 60 && !isNativeResolutionStream) {
                    if (display.getMode().getPhysicalWidth() != candidate.getPhysicalWidth() ||
                            display.getMode().getPhysicalHeight() != candidate.getPhysicalHeight()) {
                        continue;
                    }
                }

                // Make sure the resolution doesn't regress unless if it's over 60 FPS
                // where we may need to reduce resolution to achieve the desired refresh rate.
                if (resolutionReduced && !(game.prefConfig.fps > 60 && resolutionFitsStream)) {
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
                if (game.prefConfig.useTextureView || Build.VERSION.SDK_INT < Build.VERSION_CODES.S ||
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
                    if (game.prefConfig.fps <= 60) {
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

        if (game.prefConfig.useTextureView && !game.useArr) {
            // A SurfaceTexture is composed into the window, so Surface.setFrameRate() cannot vote for it.
            windowLayoutParams.preferredRefreshRate = displayRefreshRate;
            getVideoWindow().setAttributes(windowLayoutParams);
        }

        if (!game.prefConfig.useTextureView && game.prefConfig.upscalingMode == UpscalingPolicy.Mode.OFF) {
            // Keep decoder buffers at stream resolution; the compositor scales them to the view.
            game.renderView.getHolder().setFixedSize(game.prefConfig.width, game.prefConfig.height);
        }
        applyVideoScaling();

        // Set the desired refresh rate that will get passed into setFrameRate() later
        game.desiredRefreshRate = displayRefreshRate;

        if (game.getPackageManager().hasSystemFeature(PackageManager.FEATURE_TELEVISION) ||
                game.getPackageManager().hasSystemFeature(PackageManager.FEATURE_LEANBACK)) {
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

    void hideSystemUi(int delay) {
        Handler h = game.getWindow().getDecorView().getHandler();
        if (h != null) {
            h.removeCallbacks(game.hideSystemUi);
            h.postDelayed(game.hideSystemUi, delay);
        }
    }

    // Mid-stream reconfigure. A Rubylight 2.2 host that lists control message 0x5532 changes the
    // stream's size or frame rate in place and answers with an IDR. A native-resolution stream asks
    // for the panel's size when a foldable opens or closes, computed like the stream's start size,
    // and for its window's size in split screen or a desktop window. A stream at the panel's top
    // refresh rate follows a panel whose top rate changes. Fixed presets, PyroWave and decoders
    // without adaptive playback never ask.
    void scheduleReconfigureCheck() {
        scheduleReconfigureCheck(StreamReconfigureRequester.SETTLE_MS);
    }

    void scheduleReconfigureCheck(long settleMs) {
        if (!game.connected || game.isFinishing()) {
            return;
        }
        game.bitrateHandler.removeCallbacks(game.checkReconfigure);
        game.bitrateHandler.postDelayed(game.checkReconfigure, game.reconfigureRequester.onChange(SystemClock.uptimeMillis(), settleMs));
    }

    // Rotate on a host that cannot turn this stream: say once that the picture is scaled instead.
    void noteRotationWithoutReconfigure() {
        if (game.streamOrientation != StreamOrientation.ROTATE || orientationNoticeShown || !game.connected ||
                game.externalPresentation != null || game.app == null || game.app.getRole() == NvApp.Role.INPUT_ONLY ||
                (game.hostShapesStream && MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_RECONFIGURE))) {
            return;
        }
        orientationNoticeShown = true;
        game.messages.alert(R.string.stream_orientation_scaled, Toast.LENGTH_LONG);
    }

    void startReconfigureTracking() {
        displaySignature = displaySignature();
        if (game.decoderRenderer == null) {
            return;
        }
        int[] size = game.decoderRenderer.getStreamSize();
        streamStartedLandscape = size[0] >= size[1];
        int fpsMillihz = game.decoderRenderer.getStreamFrameRateMilliHz();
        if (fpsMillihz <= 0) fpsMillihz = game.streamFps * 1000;
        // What the phone offered when the stream was asked for, in the window shape and on the panel
        // it had then: a turn, fold or unfold while the stream connected was not seen, so the settled
        // check below still asks for it.
        boolean launchLandscape = launchWidth > 0 && launchHeight > 0 ? launchWidth >= launchHeight : windowLandscape();
        int[] offered = offeredStream(size[0], size[1], fpsMillihz, launchLandscape,
                launchPanel != null ? launchPanel : PreferenceConfiguration.nativeResolution(game));
        if (offered == null) {
            offered = new int[] {size[0], size[1], fpsMillihz};
        }
        game.reconfigureRequester.start(size[0], size[1], fpsMillihz, offered[0], offered[1], offered[2]);
        game.qualityMaxWidth = size[0];
        game.qualityMaxHeight = size[1];
        game.qualityMaxFps = game.streamFps;
        game.qualityMaxFrameRateMilliHz = fpsMillihz;
        // The surface was created before negotiation supplied the exact stream cadence.
        refreshSurfaceFrameRateVote();
        scheduleReconfigureCheck();
    }

    void checkStreamReconfigure() {
        if (!game.connected || game.isFinishing() || game.decoderRenderer == null || (!game.nativeResolutionStream && !game.fpsFollowsDisplay &&
                !StreamOrientation.followsRotation(game.streamOrientation, game.hostShapesStream))) {
            return;
        }
        long now = SystemClock.uptimeMillis();
        if (!game.reconfigureRequester.settled(now)) {
            game.bitrateHandler.postDelayed(game.checkReconfigure, game.reconfigureRequester.remainingMs(now));
            return;
        }
        if ((Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && game.isInPictureInPictureMode()) ||
                !MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_RECONFIGURE)) {
            return;
        }
        if (game.adaptiveQuality != null && game.pendingQualityTarget != null) {
            game.bitrateHandler.postDelayed(game.checkReconfigure, 1000);
            return;
        }
        int[] offered = offeredStream(game.adaptiveQuality == null ? game.reconfigureRequester.width() : game.qualityMaxWidth,
                game.adaptiveQuality == null ? game.reconfigureRequester.height() : game.qualityMaxHeight,
                game.adaptiveQuality == null ? game.reconfigureRequester.fpsMillihz() : game.adaptation.qualityCeilingFrameRateMilliHz());
        if (game.adaptiveQuality != null && offered != null) {
            AdaptiveQualityController.Target target = game.adaptiveQuality.resizeCeiling(offered[0], offered[1],
                    Math.round(offered[2] / 1000f), now);
            // The quality ladder uses whole FPS, but a fractional-only display change still
            // needs to reach the host when the current level uses the ceiling's rate.
            if (target == null && game.adaptiveQuality.current().fps == Math.round(offered[2] / 1000f) &&
                    game.reconfigureRequester.fpsMillihz() != offered[2]) target = game.adaptiveQuality.current();
            if (target == null || game.adaptation.applyAdaptiveQuality(target, now, offered[2])) {
                game.qualityMaxWidth = offered[0]; game.qualityMaxHeight = offered[1]; game.qualityMaxFps = Math.round(offered[2] / 1000f);
                game.qualityMaxFrameRateMilliHz = offered[2];
                game.streamPanelHz = game.panelMaxRefreshRate;
            }
            return;
        }
        int[] request = offered == null ? null : game.reconfigureRequester.decide(offered[0], offered[1], offered[2]);
        if (request == null) {
            return;
        }
        if (requestStreamReconfigure(request)) {
            // This is the new ceiling even if automatic quality is enabled later.
            game.qualityMaxWidth = request[0]; game.qualityMaxHeight = request[1]; game.qualityMaxFps = Math.round(request[2] / 1000f);
            game.qualityMaxFrameRateMilliHz = request[2];
            game.streamPanelHz = game.panelMaxRefreshRate;
        }
    }

    boolean requestStreamReconfigure(int[] request) {
        LimeLog.info("Reconfigure: asking for " + request[0] + "x" + request[1] + " at " + request[2] / 1000f +
                " fps (was " + game.reconfigureRequester.width() + "x" + game.reconfigureRequester.height() + " at " +
                game.reconfigureRequester.fpsMillihz() / 1000f + " fps)");
        if (!game.decoderRenderer.prepareForReconfigure(request[0], request[1])) {
            LimeLog.warning("Reconfigure: the decoder cannot switch to " + request[0] + "x" + request[1] +
                    "; the stream stays as it is");
            if (StreamOrientation.followsRotation(game.streamOrientation, game.hostShapesStream) && !orientationNoticeShown) {
                orientationNoticeShown = true;
                game.messages.alert(R.string.stream_orientation_decoder_limit, Toast.LENGTH_LONG);
            }
            return false;
        }
        if (!MoonBridge.requestReconfigure(request[0], request[1], request[2])) {
            LimeLog.warning("Reconfigure: request not sent");
            return false;
        }
        boolean fpsChanged = request[2] != game.reconfigureRequester.fpsMillihz();
        game.reconfigureRequester.sent(request);
        game.decoderRenderer.resetDecodeTiming();
        if (fpsChanged) setStreamFrameRate(request[2]);
        return true;
    }

    // The stream runs at this rate now: a reconfigure asked for it, or a 0x553C rate change ran.
    void setStreamFrameRate(int fpsMillihz) {
        int fps = Math.round(fpsMillihz / 1000f);
        game.streamFps = fps;
        game.prefConfig.fps = fps;
        game.decoderRenderer.setStreamFrameRateMilliHz(fpsMillihz);
        if (game.adaptiveBitrate != null) game.adaptiveBitrate.setFrameRate(fps);
        refreshSurfaceFrameRateVote();
    }

    // What this phone offers a stream that follows it, {width, height, fps in millihertz}, given
    // the stream's current values; null when this stream cannot change in place.
    private int[] offeredStream(int width, int height, int fpsMillihz) {
        return offeredStream(width, height, fpsMillihz, windowLandscape(), PreferenceConfiguration.nativeResolution(game));
    }

    // As above, for a window of that orientation on a screen whose own resolution is nativeResolution.
    private int[] offeredStream(int width, int height, int fpsMillihz, boolean windowLandscape, String nativeResolution) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return null;
        }
        int[] limits = game.decoderRenderer.getReconfigureLimits();
        if (limits == null) {
            return null;
        }
        int[] size = game.nativeResolutionStream ? nativeStreamSize(limits, windowLandscape, nativeResolution) :
                StreamOrientation.followsRotation(game.streamOrientation, game.hostShapesStream) && !game.isInMultiWindowMode() ?
                StreamOrientation.turnedSize(launchWidth, launchHeight, windowLandscape, limits[0], limits[1],
                        limits[2], limits[3]) : null;
        int fps = StreamResizePolicy.targetFpsMillihz(game.fpsFollowsDisplay, fpsMillihz, game.streamPanelHz, game.panelMaxRefreshRate);
        return size == null ? new int[] {width, height, fps} : new int[] {size[0], size[1], fps};
    }

    @TargetApi(Build.VERSION_CODES.O)
    private int[] nativeStreamSize(int[] limits, boolean windowLandscape, String nativeResolution) {
        if (game.isInMultiWindowMode()) {
            // Split screen or a desktop window: the stream fills the window's pane, in panel pixels.
            View pane = game.findViewById(android.R.id.content);
            int paneWidth = pane.getWidth() - pane.getPaddingLeft() - pane.getPaddingRight();
            int paneHeight = pane.getHeight() - pane.getPaddingTop() - pane.getPaddingBottom();
            Display display = game.getWindowManager().getDefaultDisplay();
            Point real = new Point();
            display.getRealSize(real);
            // The screen in use's own long side: a foldable can list the other panel's modes too.
            String panel = PreferenceConfiguration.nativeResolution(game);
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
                StreamOrientation.streamOrientation(game.streamOrientation, game.hostShapesStream, streamStartedLandscape,
                        windowLandscape),
                limits[0], limits[1], limits[2], limits[3]);
    }

    private boolean windowLandscape() {
        Point real = new Point();
        game.getWindowManager().getDefaultDisplay().getRealSize(real);
        return real.x >= real.y;
    }

    private String displaySignature() {
        Display display = getVideoDisplay();
        // The input pane must not hide a refresh-capability change on the video output.
        // Local output still tracks the phone's native size across folds and mode changes.
        String resolution = game.externalPresentation == null ? PreferenceConfiguration.nativeResolution(game) : "";
        return display.getDisplayId() + ":" + resolution + "@" +
                com.limelight.binding.video.DisplayFrameRatePolicy.maxRefreshRate(display);
    }

    // Display changes also fire for refresh rate switches; only a new panel or a new top refresh
    // rate matters here.
    void onDisplayModesChanged() {
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
        game.panelMaxRefreshRate = com.limelight.binding.video.DisplayFrameRatePolicy.maxRefreshRate(
                getVideoDisplay());
        scheduleReconfigureCheck();
    }

    void logDisplayRefreshPolicy(Display display) {
        logDisplayRefreshPolicy(display,
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA && display.hasArrSupport());
    }

    void logDisplayRefreshPolicy(Display display, boolean arrSupported) {
        WindowManager.LayoutParams attributes = getVideoWindow().getAttributes();
        String changed = displayRefreshDiagnostics.update(Build.VERSION.SDK_INT, display.getDisplayId(),
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.M ? display.getMode().getModeId() : 0,
                arrSupported, game.prefConfig.vrr, game.useArr,
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.M ? attributes.preferredDisplayModeId : 0,
                attributes.preferredRefreshRate, game.appSurfaceFrameRateVote);
        if (changed != null) LimeLog.info(changed);
    }

    void onPaneLaidOut(View pane) {
        if (game.externalPresentation != null) return;
        int width = pane.getWidth() - pane.getPaddingLeft() - pane.getPaddingRight();
        int height = pane.getHeight() - pane.getPaddingTop() - pane.getPaddingBottom();
        if (width == lastPaneWidth && height == lastPaneHeight) {
            return;
        }
        lastPaneWidth = width;
        lastPaneHeight = height;
        // Full-screen sizes come from the display; only a window follows its own pane.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N && game.isInMultiWindowMode()) {
            scheduleReconfigureCheck();
        }
    }

    // Frames of a new size came out of the decoder: scale, letterbox, touch and picture-in-picture
    // now follow that size.
    void applyStreamSize(int width, int height) {
        if (game.isFinishing() || game.isDestroyed() || (width == game.prefConfig.width && height == game.prefConfig.height)) {
            return;
        }
        LimeLog.info("Stream now " + width + "x" + height + " (was " + game.prefConfig.width + "x" + game.prefConfig.height + ")");
        game.prefConfig.width = width;
        game.prefConfig.height = height;
        if (game.adaptiveBitrate != null) game.adaptiveBitrate.setStreamSize(width, height);
        configureStreamWindow();
        if (game.prefConfig.upscalingMode == UpscalingPolicy.Mode.OFF) {
            if (game.prefConfig.useTextureView) {
                SurfaceTexture texture = game.renderView.getTextureView().getSurfaceTexture();
                if (texture != null) {
                    texture.setDefaultBufferSize(width, height);
                }
            } else {
                game.renderView.getHolder().setFixedSize(width, height);
            }
        }
        applyVideoScaling();
        if (game.prefConfig.touchscreenTrackpad && game.conn != null) {
            // Trackpad motion is scaled to the stream size.
            game.touchInput.cancelTouchInput();
            game.touchInput.initializeTouchContexts();
        }
        setPreferredOrientationForCurrentDisplay();
        updatePipAutoEnter();
    }

    private void refreshSurfaceFrameRateVote() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R && game.surfaceCreated && game.videoSurface != null &&
                game.videoSurface.isValid() && !game.prefConfig.useTextureView) {
            voteSurfaceFrameRate(game.videoSurface);
        }
    }

    void voteSurfaceFrameRate(Surface surface) {
        if (game.useArr && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            // The stream rate, seamless only: ARR's steps are seamless, and a 0 vote would leave
            // the game at Android's 60 Hz default for games (see arrVote). arrHalfRate watches for it.
            int exactRate = game.decoderRenderer == null ? 0 : game.decoderRenderer.getStreamFrameRateMilliHz();
            float vote = com.limelight.binding.video.DisplayFrameRatePolicy.arrVote(
                    exactRate > 0 ? exactRate / 1000f : game.prefConfig.fps, game.panelMaxRefreshRate);
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
        if (game.prefConfig.vrr) {
            desiredFrameRate = game.panelMaxRefreshRate;
        }
        else if (mayReduceRefreshRate() || game.desiredRefreshRate < game.prefConfig.fps) {
            int exactRate = game.decoderRenderer == null ? 0 : game.decoderRenderer.getStreamFrameRateMilliHz();
            desiredFrameRate = exactRate > 0 ? exactRate / 1000f : game.prefConfig.fps;
        }
        else {
            // Otherwise, we will pretend that our frame rate matches the refresh rate we picked in
            // prepareDisplayForRendering(). This will usually be the highest refresh rate that our
            // frame rate evenly divides into, which ensures the lowest possible display latency.
            desiredFrameRate = game.desiredRefreshRate;
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
            game.appSurfaceFrameRateVote = frameRate;
            appSurfaceSeamlessOnly = changeStrategy == Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS;
        }
        else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            // No strategy on API 30: the two-argument vote only ever switches seamlessly.
            surface.setFrameRate(frameRate,
                    Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE);
            game.appSurfaceFrameRateVote = frameRate;
            appSurfaceSeamlessOnly = true;
        }
    }

    void startRefreshSwitchPolling() {
        if (refreshSwitchMonitor == null) {
            refreshSwitchMonitor = new com.limelight.binding.video.RefreshSwitchMonitor(game);
        }
        game.bitrateHandler.removeCallbacks(pollRefreshSwitch);
        game.bitrateHandler.postDelayed(pollRefreshSwitch, 1000);
    }

    void stopRefreshSwitchPolling() {
        game.bitrateHandler.removeCallbacks(pollRefreshSwitch);
        arrHalfRate.reset();
        game.messages.status("refresh", null);
        game.messages.status("arr", null);
        game.messages.status("hdr", null);
    }

    // When the screen has stayed off the rate voted for, says why on the overlay's status line.
    private void checkRefreshSwitch() {
        if (refreshSwitchMonitor == null) return;
        String text = refreshSwitchMonitor.poll(SystemClock.uptimeMillis(), getVideoDisplay(),
                game.surfaceCreated && !game.useArr ? game.appSurfaceFrameRateVote : null, appSurfaceSeamlessOnly);
        if (game.messages.status("refresh", text)) game.overlays.updatePerformanceOverlay();
    }

    // Under ARR the refresh switch check above cannot tell what the screen runs at, so watch what it
    // shows: about half the rendered frames for 3 s, with the display at about half the stream rate,
    // means Android holds the stream at half its rate.
    private void checkArrRate() {
        if (!game.useArr || game.decoderRenderer == null) return;
        float[] rates = game.decoderRenderer.getFrameRates();
        float rendered = PerformanceOverlay.renderedFps(rates);
        float presented = PerformanceOverlay.presentedFps(rates);
        Display display = getVideoDisplay();
        float displayHz = display == null ? 0 : display.getRefreshRate();
        int exactRate = game.decoderRenderer.getStreamFrameRateMilliHz();
        float streamHz = exactRate > 0 ? exactRate / 1000f : game.prefConfig.fps;
        boolean was = arrHalfRate.flagged();
        boolean flagged = arrHalfRate.update(SystemClock.uptimeMillis(), rendered, presented, displayHz, streamHz);
        if (flagged && !was) {
            LimeLog.warning("VRR: ARR shows " + presented + " of " + rendered + " fps rendered for " +
                    com.limelight.binding.video.DisplayFrameRatePolicy.ArrHalfRateCheck.HOLD_MS / 1000 +
                    " s, display at " + displayHz + " Hz: the screen is held at half the stream rate (vote " +
                    game.appSurfaceFrameRateVote + " Hz)");
        } else if (!flagged && was) {
            LimeLog.info("VRR: ARR shows the rendered frames again (" + presented + " of " + rendered + " fps)");
        }
        String text = flagged ? game.getString(R.string.stream_vrr_arr_half_rate, Math.round(presented), Math.round(rendered)) : null;
        if (game.messages.status("arr", text)) game.overlays.updatePerformanceOverlay();
    }

    // Render through Vulkan can't keep up on this display: say so once per PC and on the status line.
    // The renderer stays on and the setting stays; turning it off is the user's choice.
    private void checkVulkanPace() {
        if (game.decoderRenderer != null && game.decoderRenderer.takeVulkanBehind()) {
            game.overlays.hostNotice(HostNotices.Notice.VULKAN_BEHIND, R.string.stream_vulkan_behind);
        }
    }

    // HDR was granted and then not delivered: say so on the status line for as long as it lasts,
    // and log it. A refusal before the stream is already noted by refuseHdr, not repeated here.
    private void checkHdrStatus() {
        String text = null;
        if (game.streamHdr && game.decoderRenderer != null) {
            int format = game.decoderRenderer.getActiveVideoFormat();
            HdrPolicy.Status status = format == 0 ? HdrPolicy.Status.WAITING : HdrPolicy.drawnStatus(HdrPolicy.status(true,
                    (format & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0, game.decoderRenderer.getHostHdrReport(),
                    game.decoderRenderer.getHdrMetadataState(), game.decoderRenderer.getDecodedTransfer(),
                    game.decoderRenderer.getStreamTransfer(), HdrPolicy.hlgOnly(HdrPolicy.displayHdrTypes(getVideoDisplay()))),
                    game.decoderRenderer.isVulkanHdrToneMapped());
            int res = Game.hdrStatusText(status, game.decoderRenderer.isPyroWaveHlgNeedingHostUpdate(),
                    game.decoderRenderer.isPyroWaveHlgRefusedByDisplay());
            if (status == HdrPolicy.Status.SHOWN_AS_SDR) {
                text = game.getString(res, HdrFormatPolicy.baseLayerName(game.decoderRenderer.getAnnouncedHdrFormat()));
            } else if (res != 0) {
                text = game.getString(res);
            }
            // The window took HLG before the stream but does not show it as HLG (the renderer and
            // MediaCodecDecoderRenderer log why).
            if (game.decoderRenderer.isPyroWaveHlgLost()) text = game.getString(R.string.stream_hdr_pyrowave_hlg_lost);
            // Plain words, once per PC, for what the Vulkan renderer cannot show of this screen's HDR.
            int limit = Game.vulkanHdrLimitText(game.decoderRenderer.takeVulkanHdrLimit());
            if (limit != 0) game.overlays.hostNotice(HostNotices.Notice.VULKAN_HDR_LIMIT, limit);
            // Once per stream on the status line (and once per PC as a message): the output window
            // refused HDR10+, so the picture keeps HDR10's static metadata alone.
            if (game.decoderRenderer.takeVulkanHdr10PlusRefused()) {
                LimeLog.warning("HDR10+ refused by the Vulkan renderer's window");
                game.overlays.hostNotice(HostNotices.Notice.VULKAN_HDR10_PLUS_REFUSED, R.string.stream_hdr_vulkan_no_hdr10_plus);
            }
            // And for what PyroWave cannot carry; Dolby Vision names what the stream is instead.
            int pyroWaveLimit = Game.pyroWaveHdrLimitText(game.decoderRenderer.takePyroWaveHdrLimit());
            if (pyroWaveLimit != 0) {
                game.messages.notice(game.getIntent().getStringExtra(Game.EXTRA_PC_UUID), HostNotices.Notice.PYROWAVE_HDR_LIMIT,
                        game.getString(pyroWaveLimit, HdrFormatPolicy.baseLayerName(game.decoderRenderer.getAnnouncedHdrFormat())));
            }
            if (status != lastHdrStatus) {
                lastHdrStatus = status;
                if (res != 0) LimeLog.warning("HDR requested but " + status);
            }
        }
        if (game.streamSdr10 && game.decoderRenderer != null) {
            int format = game.decoderRenderer.getActiveVideoFormat();
            boolean tenBit = (format & MoonBridge.VIDEO_FORMAT_MASK_10BIT) != 0;
            Boolean report = game.decoderRenderer.getHostHdrReport();
            if (Sdr10Policy.status(tenBit, report) == Sdr10Policy.Status.HOST_HDR) {
                text = game.getString(R.string.stream_sdr10_host_hdr);
            }
            Sdr10Policy.HostHdrStep step = Sdr10Policy.hostHdrStep(tenBit, report, sdr10LastHostHdr);
            if (step == Sdr10Policy.HostHdrStep.SAY) {
                LimeLog.warning("10-bit SDR requested but the PC sends HDR");
                game.overlays.hostNotice(HostNotices.Notice.SDR10_HOST_HDR, R.string.stream_sdr10_host_hdr);
            } else if (step == Sdr10Policy.HostHdrStep.REARM) {
                // The PC's display is in SDR now; if it goes to HDR again, say so again.
                HostNotices.reset(game, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID), HostNotices.Notice.SDR10_HOST_HDR);
            }
            if (tenBit && report != null) sdr10LastHostHdr = report;
        }
        if (game.messages.status("hdr", text)) game.overlays.updatePerformanceOverlay();
    }

    private boolean followsContentRate() {
        return com.limelight.binding.video.DisplayFrameRatePolicy.followsContentRate(Build.VERSION.SDK_INT,
                game.prefConfig.contentRateFrameRate,
                MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_CLOCK_SYNC),
                game.useArr, game.prefConfig.vrr, game.prefConfig.useTextureView);
    }

    // Moves the surface's frame rate vote to the rate the game presents at once that held steady,
    // seamless modes only, so a vote never blanks the screen during the stream.
    private void voteContentRate() {
        if (!game.surfaceCreated || game.videoSurface == null || !game.videoSurface.isValid() || game.decoderRenderer == null ||
                !followsContentRate()) {
            return;
        }
        long now = SystemClock.uptimeMillis();
        int contentMilliHz = game.decoderRenderer.getContentRateMilliHz();
        if (contentRateVote.withdraw(now, contentMilliHz)) {
            // The game no longer holds a rate: back to the vote the stream would make.
            if (keepsVrrVote(baseSurfaceFrameRate())) return;
            applySurfaceFrameRate(game.videoSurface, baseSurfaceFrameRate(), Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS);
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
        float vote = game.prefConfig.vrr ?
                com.limelight.binding.video.DisplayFrameRatePolicy.vrrContentVote(contentHz, game.panelMaxRefreshRate,
                        mayReduceRefreshRate(),
                        com.limelight.binding.video.DisplayFrameRatePolicy.seamlessRates(getVideoDisplay())) :
                com.limelight.binding.video.DisplayFrameRatePolicy.contentVote(contentHz,
                        baseSurfaceFrameRate(), mayReduceRefreshRate(), game.panelMaxRefreshRate);
        if (keepsVrrVote(vote)) return;
        applySurfaceFrameRate(game.videoSurface, vote, Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS);
        LimeLog.info("Content frame rate vote: " + vote + " Hz (game at " + contentHz + " Hz)");
        logDisplayRefreshPolicy(getVideoDisplay());
    }

    // With VRR a content vote that equals the vote in force (the panel maximum the stream started
    // with) is not made again: re-voting it seamless-only would change the reason the status line
    // gives when the screen never reached that rate (a TV that needs Match content frame rate).
    private boolean keepsVrrVote(float vote) {
        return game.prefConfig.vrr && game.appSurfaceFrameRateVote != null && Math.abs(vote - game.appSurfaceFrameRateVote) < 0.01f;
    }
}
