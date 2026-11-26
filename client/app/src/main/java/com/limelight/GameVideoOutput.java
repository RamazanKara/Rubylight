package com.limelight;

import com.limelight.binding.input.touch.TouchMode;
import com.limelight.binding.video.DisplayCapsReporter;
import com.limelight.binding.video.DisplayFrameRatePolicy;
import com.limelight.binding.video.HdrPolicy;
import com.limelight.binding.video.StreamResizePolicy;
import com.limelight.binding.video.UpscalingPolicy;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.HostDisplayChoice;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.NavigationDialogBuilder;
import com.limelight.ui.PictureInPicturePolicy;
import com.limelight.ui.StreamOrientation;
import com.limelight.ui.VideoScaling;
import com.limelight.utils.HdrBrightness;
import android.annotation.TargetApi;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowCompat;
import android.app.PictureInPictureParams;
import android.app.PendingIntent;
import android.app.RemoteAction;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.PackageManager;
import android.content.res.Configuration;
import android.graphics.Point;
import android.graphics.Rect;
import android.graphics.SurfaceTexture;
import android.graphics.drawable.Icon;
import android.media.MediaFormat;
import android.os.Build;
import android.os.Handler;
import android.util.Rational;
import android.view.Display;
import android.view.Surface;
import android.view.View;
import android.view.WindowManager;
import android.widget.TextView;
import android.widget.Toast;

/** Where and how the video is shown: the output display and window shape, picture-in-picture, scaling and zoom, orientation, the stream's size and reconfiguration, refresh rate votes and switches, HDR status and the display's capabilities. */
final class GameVideoOutput {
    private final Game game;
    final GameVideoReconfigure reconfigure;
    final GameVideoRefresh refresh;

    GameVideoOutput(Game game) {
        this.game = game;
        this.reconfigure = new GameVideoReconfigure(game, this);
        this.refresh = new GameVideoRefresh(game, this);
    }

    int launchWidth, launchHeight;
    // The screen's own resolution when the stream was asked for; null until then.
    String launchPanel;

    boolean orientationNoticeShown;
    // Keep TV video smooth (manual setting or automatic CompositorWatch probe/lock-on); null otherwise.
    com.limelight.ui.CompositorHeartbeat compositorHeartbeat;
    boolean autoCompositorKeepalive;
    boolean autoCompositorPulse;
    private final Runnable endCompositorPulse = () -> {
        autoCompositorPulse = false;
        updateCompositorHeartbeat();
    };
    // UI actions that read the decoder's limits or ask for a size and met a decoder rebuild, in the
    // order they were asked for; one per key, a newer one taking the older one's place.
    final java.util.LinkedHashMap<String, Runnable> decoderActions = new java.util.LinkedHashMap<>();
    String displaySignature = "";

    // Flags ARR showing about half the rendered frames (see checkArrRate).
    final DisplayFrameRatePolicy.ArrHalfRateCheck arrHalfRate =
            new DisplayFrameRatePolicy.ArrHalfRateCheck();
    // Whether the vote above allowed only seamless switches (the content rate vote, and every vote on API 30).
    boolean appSurfaceSeamlessOnly;

    final Runnable pollRefreshSwitch = new Runnable() {
        @Override public void run() {
            if (!game.connected || game.isFinishing()) return;
            refresh.pollRefreshSwitchTick();
            game.bitrateHandler.postDelayed(this, 1000);
        }
    };
    final Runnable pollContentRateVote = new Runnable() {
        @Override public void run() {
            if (!game.connected || game.isFinishing()) return;
            refresh.voteContentRate();
            game.bitrateHandler.postDelayed(this, 1000);
        }
    };

    static final String EXTRA_OUTPUT_DISPLAY = "rubylight.outputDisplay";

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
        String output = Integer.toString(display.getDisplayId());
        Display.Mode mode = display.getMode();
        // Foldable panels can share a logical display ID. Pixel geometry distinguishes them
        // without reacting to brightness, rotation, or an automatic refresh-rate change.
        output += ":" + Math.min(mode.getPhysicalWidth(), mode.getPhysicalHeight()) + "x" +
                Math.max(mode.getPhysicalWidth(), mode.getPhysicalHeight());
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
        Display.HdrCapabilities hdrCaps = display.getHdrCapabilities();
        if (hdrCaps != null) {
            maxNits = hdrCaps.getDesiredMaxLuminance();
            maxAverageNits = hdrCaps.getDesiredMaxAverageLuminance();
            minNits = hdrCaps.getDesiredMinLuminance();
        }
        return HdrBrightness.fromDisplay(maxNits, minNits, maxAverageNits, hdrSdrRatio(display));
    }

    void updateInputPollingRate() {
        Display display = game.getWindowManager().getDefaultDisplay();
        game.conn.setInputPollingRate(display.getMode().getRefreshRate());
    }

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
                if (game.prefConfig.touchMode != TouchMode.OFF && game.prefConfig.touchMode != TouchMode.TOUCHPAD) {
                    game.prefConfig.touchMode = TouchMode.TRACKPAD;
                }
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
        game.panelMaxRefreshRate = DisplayFrameRatePolicy.maxRefreshRate(getVideoDisplay(),
                game.prefConfig.width, game.prefConfig.height);
        if (game.prefConfig.vrr) {
            game.prefConfig.fps = DisplayFrameRatePolicy.streamFrameRate(game.panelMaxRefreshRate);
            game.prefConfig.launchRefreshRateX100 = Math.max(100, Math.min(100000, Math.round(game.panelMaxRefreshRate * 100)));
        }
        game.fpsFollowsDisplay = DisplayFrameRatePolicy.fpsFollowsDisplay(game.prefConfig.vrr,
                game.prefConfig.fps, DisplayFrameRatePolicy.maxRefreshRate(getVideoDisplay()));
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
                game.streamConnection.stopConnection();
            } else game.finish();
        }
    }

    void toggleDesktopZoom() {
        game.desktopGestures.setEnabled(!game.desktopGestures.enabled());
        if (game.trackpadPointer != null) game.trackpadPointer.syncCursor();
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
        DualScreen.saveSettings(game, editor -> VideoScaling.save(editor, mode));
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

    /**
     * Settings › Keep TV video smooth: the dot runs while the stream is shown in the foreground, in the window
     * that holds the video (an external display's while the video is there). Paused, in picture-in-picture, input
     * only, after the stream or with the setting off it is gone. Read from the stored value on any device.
     */
    void updateCompositorHeartbeat() {
        if (android.os.Looper.myLooper() != android.os.Looper.getMainLooper()) {
            game.runOnUiThread(this::updateCompositorHeartbeat);
            return;
        }
        android.view.ViewParent parent = game.renderView == null ? null : game.renderView.getParent();
        android.view.ViewGroup holder = parent instanceof android.view.ViewGroup ? (android.view.ViewGroup) parent : null;
        android.view.SurfaceView surfaceView = game.renderView == null ? null : game.renderView.getSurfaceView();
        boolean hasVideoView = surfaceView != null
                || (game.renderView != null && game.renderView.getTextureView() != null);
        boolean activeWindow = game.foreground && !game.isInPictureInPictureMode()
                && (!hasVideoView || game.surfaceCreated);
        if (!activeWindow && game.decoderRenderer != null) {
            game.decoderRenderer.resetCompositorWatchWindow();
            if (!game.decoderRenderer.isCompositorKeepaliveLockedOn()) {
                autoCompositorKeepalive = false;
                autoCompositorPulse = false;
                game.bitrateHandler.removeCallbacks(endCompositorPulse);
            }
        }
        boolean keepalive = game.prefConfig != null
                && (game.prefConfig.tvDisplayCompatibility || autoCompositorKeepalive || autoCompositorPulse);
        boolean show = holder != null && keepalive && refresh.refreshHeartbeat == null
                && game.connected && activeWindow && !game.isDestroyed()
                && (game.app == null || game.app.getRole() != NvApp.Role.INPUT_ONLY);
        if (compositorHeartbeat != null && (!show || !compositorHeartbeat.matchesStreamTarget(holder, surfaceView))) {
            compositorHeartbeat.stop();
            compositorHeartbeat = null;
        }
        if (show && compositorHeartbeat == null) {
            compositorHeartbeat = com.limelight.ui.CompositorHeartbeat.forStream(holder, surfaceView);
            compositorHeartbeat.start();
        }
        if (refresh.refreshHeartbeat != null && !refresh.canKeepRefreshHeartbeat()) {
            refresh.stopRefreshHeartbeat();
        }
    }

    void pulseCompositorKeepalive(long durationMs) {
        if (android.os.Looper.myLooper() != android.os.Looper.getMainLooper()) {
            game.runOnUiThread(() -> pulseCompositorKeepalive(durationMs));
            return;
        }
        autoCompositorPulse = true;
        game.bitrateHandler.removeCallbacks(endCompositorPulse);
        game.bitrateHandler.postDelayed(endCompositorPulse, durationMs);
        updateCompositorHeartbeat();
    }

    void setAutoCompositorKeepalive(boolean active) {
        if (android.os.Looper.myLooper() != android.os.Looper.getMainLooper()) {
            game.runOnUiThread(() -> setAutoCompositorKeepalive(active));
            return;
        }
        if (autoCompositorKeepalive == active) {
            return;
        }
        autoCompositorKeepalive = active;
        updateCompositorHeartbeat();
    }

    void clearAutoCompositorKeepalive() {
        if (android.os.Looper.myLooper() != android.os.Looper.getMainLooper()) {
            game.runOnUiThread(this::clearAutoCompositorKeepalive);
            return;
        }
        autoCompositorKeepalive = false;
        autoCompositorPulse = false;
        game.bitrateHandler.removeCallbacks(endCompositorPulse);
        updateCompositorHeartbeat();
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
                game.isInMultiWindowMode();
        boolean largeScreen = game.getResources().getConfiguration().smallestScreenWidthDp >= 600 ||
                game.getPackageManager().hasSystemFeature("android.hardware.type.pc");
        boolean squarish = !freeWindow && !largeScreen &&
                PreferenceConfiguration.isSquarishScreen(game.getWindowManager().getDefaultDisplay());
        PreferenceConfiguration config = game.prefConfig != null ? game.prefConfig : new PreferenceConfiguration();
        game.setRequestedOrientation(game.safeArea.requested(StreamOrientation.requested(game.streamOrientation, freeWindow, largeScreen, squarish,
                config.onscreenController, game.keepsRequestedShape, config.width, config.height), freeWindow));
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

    // What the decoder's maximum covers from the start (see StreamResizePolicy.coveringSize), after
    // startInWindowShape: the turned launch size with Rotate on a host that shapes the stream, and
    // on a foldable (hinge sensor) streaming its own resolution, its panels (coveringPanels).
    int[] coveringStreamSize() {
        boolean turns = StreamOrientation.followsRotation(game.streamOrientation, game.hostShapesStream);
        int[][] panels = null;
        if (game.nativeResolutionStream && game.externalPresentation == null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.R &&
                game.getPackageManager().hasSystemFeature(PackageManager.FEATURE_SENSOR_HINGE_ANGLE)) {
            panels = foldPanels(getVideoDisplay());
        }
        return StreamResizePolicy.coveringSize(launchWidth, launchHeight, turns, panels);
    }

    // A foldable's (hinge sensor) panels' own sizes, the panel not in use included; null elsewhere.
    int[][] foldPanelSizes() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R ||
                !game.getPackageManager().hasSystemFeature(PackageManager.FEATURE_SENSOR_HINGE_ANGLE)) {
            return null;
        }
        return PreferenceConfiguration.panelSizes(foldPanels(game.getWindowManager().getDefaultDisplay()));
    }

    // The panel sizes a fold or unfold may move the stream to: the video display's modes and, from
    // Android 16 QPR2, every built-in display's, the panel not in use included.
    @androidx.annotation.RequiresApi(Build.VERSION_CODES.R)
    private int[][] foldPanels(Display video) {
        java.util.Map<Integer, Display> byId = new java.util.LinkedHashMap<>();
        java.util.Set<Integer> builtInIds = new java.util.HashSet<>();
        byId.put(video.getDisplayId(), video);
        android.hardware.display.DisplayManager manager =
                (android.hardware.display.DisplayManager) game.getSystemService(Context.DISPLAY_SERVICE);
        if (manager != null) {
            for (Display display : manager.getDisplays()) byId.putIfAbsent(display.getDisplayId(), display);
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA &&
                    Build.VERSION.SDK_INT_FULL >= Build.VERSION_CODES_FULL.BAKLAVA_1) {
                for (Display display : manager.getDisplays(android.hardware.display.DisplayManager.DISPLAY_CATEGORY_BUILT_IN_DISPLAYS)) {
                    builtInIds.add(display.getDisplayId());
                    byId.putIfAbsent(display.getDisplayId(), display);
                }
            }
        }
        java.util.List<Display> displays = new java.util.ArrayList<>(byId.values());
        int[] ids = new int[displays.size()];
        boolean[] builtIn = new boolean[ids.length];
        int[][][] modes = new int[ids.length][][];
        StringBuilder seen = new StringBuilder();
        for (int i = 0; i < ids.length; i++) {
            Display display = displays.get(i);
            ids[i] = display.getDisplayId();
            builtIn[i] = builtInIds.contains(ids[i]);
            Display.Mode[] supported = display.getSupportedModes();
            java.util.Set<String> sizes = new java.util.LinkedHashSet<>();
            modes[i] = new int[supported.length][];
            for (int m = 0; m < supported.length; m++) {
                modes[i][m] = new int[] {supported[m].getPhysicalWidth(), supported[m].getPhysicalHeight()};
                sizes.add(modes[i][m][0] + "x" + modes[i][m][1]);
            }
            seen.append(i == 0 ? "" : ", ").append("display ").append(ids[i]).append(builtIn[i] ? " built in " : " ")
                    .append(sizes);
        }
        LimeLog.info("Fold panels from " + seen);
        return StreamResizePolicy.coveringPanels(video.getDisplayId(), ids, builtIn, modes);
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
                game.isInMultiWindowMode());

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

    private boolean shouldIgnoreInsetsForResolution(int width, int height) {
        Display display = game.getWindowManager().getDefaultDisplay();
        for (Display.Mode candidate : display.getSupportedModes()) {
            // Ignore insets if this is an exact match for the display resolution
            if ((width == candidate.getPhysicalWidth() && height == candidate.getPhysicalHeight()) ||
                    (height == candidate.getPhysicalWidth() && width == candidate.getPhysicalHeight())) {
                return true;
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
        return game.isInMultiWindowMode() && !game.windowCoversDisplay();
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
        // Only the user's fill/follow choice or an exact panel size spans a notch, a preset one
        // included (nativeResolutionStream leaves that out); the cutout-safe entry keeps clear of it.
        return game.prefConfig != null && !VideoScaling.keepsCornersClear(game.prefConfig.videoScaling) && (VideoScaling.coversArea(game.prefConfig.videoScaling) ||
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

    /**
     * Whether the stream may take the lowest refresh that fits its frame rate instead of the top whole
     * multiple (60 Hz instead of 120 for 60 fps) and vote its own rate: Lower refresh rate alone, whose
     * Off keeps the top rate. Moonlight let its FPS-capped and smoothest pacings lower it too; those
     * pacings show every frame, each for two refreshes at twice the frame rate. The display mode
     * choice, the surface votes and, through the chosen mode, Balanced with FPS limit's cap
     * (FramePacingPolicy.limitedFrameRate) follow this. A top rate that would judder still gives way
     * without it (60 Hz for 60 fps on a 90 Hz screen, DisplayModeChoice).
     */
    boolean mayReduceRefreshRate() {
        return game.prefConfig.reduceRefreshRate;
    }

    boolean isTelevision() {
        return game.getPackageManager().hasSystemFeature(PackageManager.FEATURE_TELEVISION) ||
                game.getPackageManager().hasSystemFeature(PackageManager.FEATURE_LEANBACK);
    }

    void hideSystemUi(int delay) {
        Handler h = game.getWindow().getDecorView().getHandler();
        if (h != null) {
            h.removeCallbacks(game.hideSystemUi);
            h.postDelayed(game.hideSystemUi, delay);
        }
    }

    // Frames of a new size came out of the decoder: scale, letterbox, touch and picture-in-picture
    // now follow that size.
    void applyStreamSize(int width, int height) {
        game.streamWidth = width;
        game.streamHeight = height;
        if (game.isFinishing() || game.isDestroyed()) {
            return;
        }
        // A stream menu left open names the new size in its header.
        game.menuActions.refreshStreamMenu();
        if (width == game.prefConfig.width && height == game.prefConfig.height) {
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

    float prepareDisplayForRendering() { return refresh.prepareDisplayForRendering(); }
    void logDisplayRefreshPolicy(Display display) { refresh.logDisplayRefreshPolicy(display); }
    void logDisplayRefreshPolicy(Display display, boolean arrSupported) { refresh.logDisplayRefreshPolicy(display, arrSupported); }
    void voteSurfaceFrameRate(Surface surface) { refresh.voteSurfaceFrameRate(surface); }
    void startRefreshSwitchPolling() { refresh.startRefreshSwitchPolling(); }
    void stopRefreshSwitchPolling() { refresh.stopRefreshSwitchPolling(); }
    void armHdrRecomposition() { refresh.armHdrRecomposition(); }
    private void checkHdrStatus() { refresh.checkHdrStatus(); }
    private void voteContentRate() { refresh.voteContentRate(); }

    void scheduleReconfigureCheck() { reconfigure.scheduleReconfigureCheck(); }
    void scheduleReconfigureCheck(long settleMs) { reconfigure.scheduleReconfigureCheck(settleMs); }
    void noteRotationWithoutReconfigure() { reconfigure.noteRotationWithoutReconfigure(); }
    void startReconfigureTracking() { reconfigure.startReconfigureTracking(); }
    void listenForRebuilds() { reconfigure.listenForRebuilds(); }
    void whenDecoderReady(String key, Runnable action) { reconfigure.whenDecoderReady(key, action); }
    void dropDecoderAction(String key) { reconfigure.dropDecoderAction(key); }
    void clearDecoderActions() { reconfigure.clearDecoderActions(); }
    void checkStreamReconfigure() { reconfigure.checkStreamReconfigure(); }
    boolean requestStreamReconfigure(int[] request) { return reconfigure.requestStreamReconfigure(request); }
    void setStreamFrameRate(int fpsMillihz) { reconfigure.setStreamFrameRate(fpsMillihz); }
    private String displaySignature() { return reconfigure.displaySignature(); }
    void onDisplayModesChanged() { reconfigure.onDisplayModesChanged(); }
    void onPaneLaidOut(View pane) { reconfigure.onPaneLaidOut(pane); }
}
