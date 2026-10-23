package com.limelight.demo;

import android.app.Activity;
import android.app.PictureInPictureParams;
import android.content.Context;
import android.content.pm.PackageManager;
import android.content.res.AssetFileDescriptor;
import android.content.res.Configuration;
import android.graphics.Rect;
import android.media.MediaCodecInfo;
import android.media.MediaCodecList;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.util.Rational;
import android.util.TypedValue;
import android.view.Gravity;
import android.util.Log;
import android.view.KeyEvent;
import android.view.SurfaceHolder;
import android.view.View;
import android.view.WindowManager;
import android.widget.FrameLayout;
import android.widget.TextView;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.R;
import com.limelight.binding.input.virtual_controller.VirtualController;
import com.limelight.binding.video.PerfOverlayListener;
import com.limelight.binding.video.PerformanceOverlay;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.binding.input.keyboard.PcKeyboardController;
import com.limelight.ui.PcKeyboardOverlay;
import com.limelight.ui.StreamMenu;
import com.limelight.ui.StreamMenuHint;
import com.limelight.ui.AdaptiveLayout;
import com.limelight.ui.StreamWindow;
import com.limelight.ui.StreamView;
import com.limelight.ui.VideoScaling;

import java.io.IOException;

public final class DemoGameActivity extends Activity implements SurfaceHolder.Callback, PerfOverlayListener,
        StreamWindow {
    static volatile boolean forceH264;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private StreamView stream;
    private TextView overlay, label;
    private VirtualController controls;
    private com.limelight.ui.GameTouchControls gameControls;
    private DemoSamplePlayer player;
    private String mode;
    private boolean showControls, prepared, started, fallback, microphoneMuted;
    private long positionUs;
    private long tick;
    private StreamMenu menu;
    private com.limelight.console.NativeMenuController radeon;
    private java.util.concurrent.ScheduledExecutorService radeonThread;
    private DemoTouchpad touchpad;
    private PcKeyboardOverlay pcKeyboard;
    private String fit;
    private int alignment;
    // The bundled samples are 1920x1080.
    private static final int SAMPLE_WIDTH = 1920, SAMPLE_HEIGHT = 1080;
    // The stream size the view is laid out for: the sample's, or "--es size 2184x1968" to stand in for
    // a stream of the panel's own size (the sample is then scaled into it).
    private int videoWidth = SAMPLE_WIDTH, videoHeight = SAMPLE_HEIGHT;
    private final Runnable metrics = new Runnable() {
        @Override public void run() {
            DemoMetrics.update(DemoGameActivity.this, DemoGameActivity.this, tick++);
            handler.postDelayed(this, 2000);
        }
    };

    @Override protected void attachBaseContext(Context base) {
        super.attachBaseContext(new DemoContext(base));
    }

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN | WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        getWindow().getDecorView().setSystemUiVisibility(View.SYSTEM_UI_FLAG_FULLSCREEN |
                View.SYSTEM_UI_FLAG_HIDE_NAVIGATION | View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY);
        // The sample has no text input; keep the soft keyboard (and its navigation-bar buttons) off the stream.
        getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_STATE_ALWAYS_HIDDEN);
        setContentView(R.layout.activity_game);
        mode = PerformanceOverlay.normalizeMode(state == null ? getIntent().getStringExtra("state") : state.getString("mode"));
        showControls = state == null ? "touch".equals(getIntent().getStringExtra("state")) : state.getBoolean("controls");
        positionUs = state == null ? 0 : state.getLong("positionUs");
        stream = findViewById(R.id.surfaceView);
        // "--es fit stretch", "fill" or "native" pick that video scaling, "--es fit safe" keeps the
        // cutout-safe area like the inset-excluded Native preset; the default is a native full-panel
        // stream. "--es align top" moves a Fit or 1:1 picture to the top.
        fit = getIntent().getStringExtra("fit");
        alignment = "top".equals(getIntent().getStringExtra("align")) ? VideoScaling.TOP : VideoScaling.CENTER;
        String size = getIntent().getStringExtra("size");
        if (size != null && size.matches("\\d+x\\d+")) {
            videoWidth = Integer.parseInt(size.substring(0, size.indexOf('x')));
            videoHeight = Integer.parseInt(size.substring(size.indexOf('x') + 1));
        }
        applyScaling();
        // "--es orientation landscape", "portrait" or "rotate" applies that Orientation setting to the
        // landscape sample, which keeps its size like a stream on a host that cannot turn it.
        String orientation = getIntent().getStringExtra("orientation");
        if (orientation != null) {
            setRequestedOrientation(com.limelight.ui.StreamOrientation.requested(
                    com.limelight.ui.StreamOrientation.fromValue(orientation), false, false, false, false, false, 1920, 1080));
        }
        // The same window setup and safe-area handling as Game, so cutout and fold layouts can be checked here.
        androidx.core.view.WindowCompat.setDecorFitsSystemWindows(getWindow(), false);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            getWindow().getAttributes().layoutInDisplayCutoutMode =
                    WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS;
        }
        AdaptiveLayout.attach(this, findViewById(android.R.id.content));
        stream.initializeSurface(false);
        stream.getHolder().addCallback(this);
        overlay = findViewById(R.id.performanceOverlay);
        findViewById(R.id.performanceOverlayScroll).setVisibility(View.VISIBLE);
        placeOverlay();
        overlay.setOnLongClickListener(view -> {
            // Compact, Statistics, Debug, then Compact again.
            setMode(PerformanceOverlay.MODE_COMPACT.equals(mode) ? PerformanceOverlay.MODE_STATISTICS :
                    PerformanceOverlay.MODE_STATISTICS.equals(mode) ? PerformanceOverlay.MODE_DEBUG : PerformanceOverlay.MODE_COMPACT);
            return true;
        });
        String touch = getIntent().getStringExtra("touch");
        if (touch != null) {
            // Fingers drive the trackpad; Back opens the menu.
            touchpad = new DemoTouchpad(this, stream, findViewById(R.id.localCursor), touch);
            findViewById(R.id.backgroundTouchView).setOnTouchListener(touchpad);
            stream.setOnTouchListener(touchpad);
        } else {
            findViewById(R.id.backgroundTouchView).setOnClickListener(view -> showMenu());
            stream.setOnClickListener(view -> showMenu());
        }
        FrameLayout container = (FrameLayout) stream.getParent();
        if (getIntent().getBooleanExtra("pckeyboard", false)) {
            // The production PC keyboard over the sample; what it would send to a host goes to logcat (tag
            // NightjarDemoInput), and D-pad / gamepad key events from adb steer it as on a device.
            pcKeyboard = new PcKeyboardOverlay(this, "auto", 50, 90,
                    (vk, down, modifiers) -> Log.i("NightjarDemoInput", "key " + Integer.toHexString(vk) + (down ? " down" : " up")
                            + " modifiers " + modifiers),
                    () -> {
                        pcKeyboard.releaseInputs();
                        pcKeyboard.setVisibility(View.GONE);
                    });
            container.addView(pcKeyboard, new FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT,
                    FrameLayout.LayoutParams.WRAP_CONTENT, Gravity.BOTTOM));
        }
        controls = new VirtualController(null, container, this);
        com.limelight.ui.OverlayPlacement.attach(findViewById(R.id.performanceOverlayScroll), container,
                out -> controls.collectVisibleBounds(out));
        // The per-game layout editor with a demo profile; what the controls send goes to the log.
        gameControls = new com.limelight.ui.GameTouchControls(this, "demo", new com.limelight.binding.input.HeldInputState.Sink() {
            @Override public void key(int key, boolean down) { android.util.Log.i("DemoGameControls", "key " + key + " " + down); }
            @Override public void mouse(int button, boolean down) { android.util.Log.i("DemoGameControls", "mouse " + button + " " + down); }
            @Override public void gamepad(int buttons, short leftX, short leftY, short rightX, short rightY,
                                          byte leftTrigger, byte rightTrigger) {
                android.util.Log.i("DemoGameControls", "gamepad " + Integer.toHexString(buttons) + " " + leftX + "," + leftY +
                        " " + rightX + "," + rightY + " " + (leftTrigger & 0xFF) + "," + (rightTrigger & 0xFF));
            }
        }, () -> menu == null, android.app.Dialog::show);
        gameControls.setStreamTouchListener((view, event) -> {
            if (event.getActionMasked() == android.view.MotionEvent.ACTION_UP) {
                view.performClick();
                showMenu();
            }
            return true;
        });
        container.addView(gameControls, new FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT,
                FrameLayout.LayoutParams.MATCH_PARENT));
        controls.refreshLayout();
        controls.setOpacity(65);
        if (showControls) controls.show(); else controls.hide();
        label = new TextView(this);
        label.setText(R.string.demo_sample);
        label.setTextColor(0xffbcc8ce);
        label.setTextSize(11);
        label.setPadding(8, 8, 8, 12);
        FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(-2, -2, Gravity.BOTTOM | Gravity.CENTER_HORIZONTAL);
        container.addView(label, params);
        DemoMetrics.update(this, this, 0);
        // The first-stream hint, as a real stream shows it once; "--ez hint true" shows it until tapped.
        TextView hint = findViewById(R.id.streamMenuHint);
        hint.setOnClickListener(view -> {
            hint.setVisibility(View.GONE);
            StreamMenuHint.markSeen(this);
        });
        hint.getViewTreeObserver().addOnGlobalLayoutListener(() -> {
            if (hint.getVisibility() == View.VISIBLE) StreamMenuHint.place(hint, findViewById(R.id.performanceOverlayScroll));
        });
        // "--es page main|native" opens the stream menu (and its native page) without a tap.
        if (getIntent().getStringExtra("page") != null) handler.postDelayed(this::showMenu, 1500);
        boolean forced = getIntent().getBooleanExtra("hint", false);
        if (state == null && (forced || !StreamMenuHint.seen(this))) {
            handler.postDelayed(() -> {
                String text = StreamMenuHint.text(this, StreamMenuHint.hasGamepad());
                hint.setText(text);
                hint.setContentDescription(text + " " + getString(R.string.stream_menu_hint_close));
                hint.setVisibility(View.VISIBLE);
                if (!forced) handler.postDelayed(() -> {
                    hint.setVisibility(View.GONE);
                    StreamMenuHint.markSeen(this);
                }, StreamMenuHint.SHOW_MS);
            }, 1500);
        }
    }

    private void setMode(String value) {
        mode = value;
        placeOverlay();
        findViewById(R.id.performanceOverlayScroll).scrollTo(0, 0);
        DemoMetrics.update(this, this, tick);
    }

    // The same sizes as a real stream; OverlayPlacement picks the corner.
    private void placeOverlay() {
        overlay.setTextSize(TypedValue.COMPLEX_UNIT_SP, PerformanceOverlay.MODE_COMPACT.equals(mode) ? 13 :
                PerformanceOverlay.MODE_DEBUG.equals(mode) ? 11 : 12);
    }

    @Override public void onPerfUpdate(String video, String network, String decode, CharSequence statisticsText,
                                       CharSequence compactText) {
        overlay.setText(PerformanceOverlay.MODE_DEBUG.equals(mode) ? PerformanceOverlay.withHardwareMetrics(this,
                PerformanceOverlay.debugText(this, video, network, decode), DemoMetrics.hardware(tick), true) :
                PerformanceOverlay.MODE_STATISTICS.equals(mode) ? statisticsText : compactText);
        overlay.setContentDescription(PerformanceOverlay.MODE_COMPACT.equals(mode) ?
                PerformanceOverlay.describeCompact(this, compactText) : null);
    }

    // Living-room PC's Rubylight host, so the demo shows the stream menu's full layout.
    private StreamMenu.Options menuOptions() {
        ComputerDetails host = new ComputerDetails();
        DemoFixtures.describeHost(host);
        host.name = "Living-room PC";
        StreamMenu.Options options = new StreamMenu.Options(NvApp.Role.STREAM, host);
        options.overlayShown = findViewById(R.id.performanceOverlayScroll).getVisibility() == View.VISIBLE;
        options.overlayMode = mode;
        options.onscreenControls = showControls;
        options.scalingAvailable = true;
        options.pictureInPicture = Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
                getPackageManager().hasSystemFeature(PackageManager.FEATURE_PICTURE_IN_PICTURE);
        options.automaticQuality = true;
        // As for a host that takes this device's microphone; the demo records nothing.
        options.microphoneOffered = true;
        options.microphoneMuted = microphoneMuted;
        options.webUiAddress = true;
        options.gameControls = true;
        options.nativeHostControl = DemoConsole.hostControl;
        options.connection = new com.limelight.console.ConnectionInfo(host.name, MoonBridge.VIDEO_FORMAT_H265_MAIN10,
                SAMPLE_WIDTH, SAMPLE_HEIGHT, 120, 80_000, null, 4);
        return options;
    }

    private void showMenu() {
        if (menu != null) return;
        findViewById(R.id.streamMenuHint).setVisibility(View.GONE);
        StreamMenuHint.markSeen(this);
        menu = new StreamMenu(this, this::menuOptions, this::onMenuItem, DemoFixtures.nativeApiHost ? radeonController() : null);
        menu.sheet().setOnDismissListener(dialog -> menu = null);
        menu.show();
        if ("native".equals(getIntent().getStringExtra("page")) && menu.row(StreamMenu.Item.NATIVE_PAGE) != null) {
            menu.row(StreamMenu.Item.NATIVE_PAGE).performClick();
        }
    }

    private com.limelight.console.NativeMenuController radeonController() {
        if (radeon == null) {
            radeonThread = java.util.concurrent.Executors.newSingleThreadScheduledExecutor();
            radeon = new com.limelight.console.NativeMenuController(DemoConsole::client,
                    (task, delayMs) -> radeonThread.schedule(task, delayMs, java.util.concurrent.TimeUnit.MILLISECONDS),
                    handler::post);
        }
        return radeon;
    }

    // The demo has no host: it plays the parts it can show and says so for the rest.
    private void onMenuItem(StreamMenu.Item item) {
        View overlayScroll = findViewById(R.id.performanceOverlayScroll);
        switch (item) {
            case OVERLAY:
                overlayScroll.setVisibility(overlayScroll.getVisibility() == View.VISIBLE ? View.GONE : View.VISIBLE);
                return;
            case MICROPHONE:
                microphoneMuted = !microphoneMuted;
                return;
            default:
                break;
        }
        if (menu != null) menu.sheet().dismiss();
        switch (item) {
            case OVERLAY_COMPACT: setMode(PerformanceOverlay.MODE_COMPACT); overlayScroll.setVisibility(View.VISIBLE); break;
            case OVERLAY_STATISTICS: setMode(PerformanceOverlay.MODE_STATISTICS); overlayScroll.setVisibility(View.VISIBLE); break;
            case OVERLAY_DEBUG: setMode(PerformanceOverlay.MODE_DEBUG); overlayScroll.setVisibility(View.VISIBLE); break;
            case ONSCREEN_CONTROLS:
                showControls = !showControls;
                if (showControls) controls.show(); else controls.hide();
                // Showing controls changes no layout; ask for one so the overlay moves clear of them.
                overlayScroll.requestLayout();
                break;
            case VIDEO_SCALING:
                // The demo cycles through the modes instead of showing the picker.
                fit = VideoScaling.value((scalingMode() + 1) % (VideoScaling.NATIVE + 1));
                applyScaling();
                androidx.core.view.ViewCompat.requestApplyInsets(findViewById(android.R.id.content));
                break;
            case PICTURE_IN_PICTURE: enterPip(); break;
            case GAME_CONTROLS: gameControls.showMenu(); break;
            case NATIVE_RESUME: break;
            case DISCONNECT:
            case END_HOST_SESSION:
            case CLOSE_GAME:
            case QUIT_GAME: finish(); break;
            default:
                android.widget.Toast.makeText(this, R.string.demo_needs_host, android.widget.Toast.LENGTH_SHORT).show();
                break;
        }
    }

    @Override public void onBackPressed() {
        // As in a real stream, Back opens the stream menu; Disconnect leaves.
        showMenu();
    }

    private String sample() {
        // Launching the demo with "--es codec h264" forces the 8-bit sample for this process; software
        // AV1/HEVC 10-bit playback renders corrupt on some emulators.
        if (!fallback && !forceH264) {
            for (String[] candidate : new String[][] {{"video/av01", "av1"}, {"video/hevc", "hevc"}}) {
                for (MediaCodecInfo info : new MediaCodecList(MediaCodecList.REGULAR_CODECS).getCodecInfos()) {
                    if (info.isEncoder()) continue;
                    for (String type : info.getSupportedTypes()) {
                        if (candidate[0].equalsIgnoreCase(type)) {
                            try (AssetFileDescriptor ignored = getAssets().openFd("demo/stream-" + candidate[1] + ".mp4")) {
                                return candidate[1];
                            } catch (IOException ignored) { }
                        }
                    }
                }
            }
        }
        return "h264";
    }

    private void play(SurfaceHolder holder) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) {
            // The paced player needs MediaCodec callbacks (Android 6).
            label.setText(R.string.demo_playback_error);
            return;
        }
        String codec = sample();
        player = new DemoSamplePlayer(getWindowManager().getDefaultDisplay().getRefreshRate(),
                () -> playbackFailed(holder, codec));
        try (AssetFileDescriptor asset = getAssets().openFd("demo/stream-" + codec + ".mp4")) {
            player.start(asset, holder.getSurface(), positionUs);
            prepared = true;
            player.setRunning(started);
        } catch (IOException | RuntimeException e) {
            playbackFailed(holder, codec);
        }
    }

    private void playbackFailed(SurfaceHolder holder, String codec) {
        releasePlayer();
        if (!"h264".equals(codec)) {
            fallback = true;
            play(holder);
        } else {
            label.setText(R.string.demo_playback_error);
        }
    }

    private int scalingMode() {
        for (int mode = VideoScaling.FIT; mode <= VideoScaling.NATIVE; mode++) {
            if (VideoScaling.value(mode).equals(fit)) return mode;
        }
        return VideoScaling.FIT;
    }

    private void applyScaling() {
        stream.setVideoScaling(scalingMode(), alignment, videoWidth, videoHeight);
    }

    private void releasePlayer() {
        if (player != null) {
            if (prepared) positionUs = player.positionUs();
            player.release();
            player = null;
        }
        prepared = false;
    }

    @Override public void surfaceCreated(SurfaceHolder holder) { play(holder); }
    @Override public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) { }
    @Override public void surfaceDestroyed(SurfaceHolder holder) { releasePlayer(); }

    @Override protected void onPause() {
        super.onPause();
        gameControls.releaseInput();
    }

    @Override public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (!hasFocus) gameControls.releaseInput();
    }

    @Override protected void onStart() {
        super.onStart();
        started = true;
        handler.post(metrics);
        if (prepared) player.setRunning(true);
    }

    @Override protected void onStop() {
        started = false;
        handler.removeCallbacksAndMessages(null);
        if (prepared) player.setRunning(false);
        controls.releaseInput();
        if (touchpad != null) touchpad.stop();
        if (menu != null) menu.sheet().dismiss();
        super.onStop();
    }

    @Override public boolean dispatchKeyEvent(KeyEvent event) {
        PcKeyboardController.Pad pad = pcKeyboard != null && pcKeyboard.getVisibility() == View.VISIBLE
                ? padFor(event.getKeyCode()) : null;
        if (pad == null || event.getRepeatCount() > 0) return super.dispatchKeyEvent(event);
        pcKeyboard.controller().pad(pad, event.getAction() == KeyEvent.ACTION_DOWN, event.getEventTime());
        return true;
    }

    private static PcKeyboardController.Pad padFor(int keyCode) {
        switch (keyCode) {
            case KeyEvent.KEYCODE_DPAD_UP: return PcKeyboardController.Pad.UP;
            case KeyEvent.KEYCODE_DPAD_DOWN: return PcKeyboardController.Pad.DOWN;
            case KeyEvent.KEYCODE_DPAD_LEFT: return PcKeyboardController.Pad.LEFT;
            case KeyEvent.KEYCODE_DPAD_RIGHT: return PcKeyboardController.Pad.RIGHT;
            case KeyEvent.KEYCODE_DPAD_CENTER:
            case KeyEvent.KEYCODE_BUTTON_A: return PcKeyboardController.Pad.A;
            case KeyEvent.KEYCODE_BUTTON_B: return PcKeyboardController.Pad.B;
            case KeyEvent.KEYCODE_BUTTON_X: return PcKeyboardController.Pad.X;
            case KeyEvent.KEYCODE_BUTTON_Y: return PcKeyboardController.Pad.Y;
            case KeyEvent.KEYCODE_BUTTON_L1: return PcKeyboardController.Pad.LB;
            case KeyEvent.KEYCODE_BUTTON_R1: return PcKeyboardController.Pad.RB;
            case KeyEvent.KEYCODE_BUTTON_START: return PcKeyboardController.Pad.START;
            default: return null;
        }
    }

    @Override protected void onDestroy() {
        handler.removeCallbacksAndMessages(null);
        if (radeonThread != null) radeonThread.shutdownNow();
        controls.hide();
        releasePlayer();
        super.onDestroy();
    }

    @Override protected void onSaveInstanceState(Bundle state) {
        state.putString("mode", mode);
        state.putBoolean("controls", showControls);
        state.putLong("positionUs", prepared ? player.positionUs() : positionUs);
        super.onSaveInstanceState(state);
    }

    @Override public void onUserLeaveHint() {
        super.onUserLeaveHint();
        enterPip();
    }

    @Override public boolean onPictureInPictureRequested() { return enterPip(); }

    private boolean enterPip() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O || !prepared ||
                !getPackageManager().hasSystemFeature(PackageManager.FEATURE_PICTURE_IN_PICTURE)) return false;
        if (isInPictureInPictureMode()) return true;
        Rect rect = new Rect();
        stream.getGlobalVisibleRect(rect);
        return enterPictureInPictureMode(new PictureInPictureParams.Builder()
                .setAspectRatio(new Rational(16, 9)).setSourceRectHint(rect).build());
    }

    @Override public void onPictureInPictureModeChanged(boolean pip, Configuration configuration) {
        super.onPictureInPictureModeChanged(pip, configuration);
        findViewById(R.id.performanceOverlayScroll).setVisibility(pip ? View.GONE : View.VISIBLE);
        label.setVisibility(pip ? View.GONE : View.VISIBLE);
        if (pip || !showControls) controls.hide(); else controls.show();
        gameControls.hideForPip(pip);
    }

    @Override public boolean usesFullDisplayForVideo() {
        return !"safe".equals(fit);
    }

    @Override public boolean isWindowedStream() {
        androidx.window.layout.WindowMetricsCalculator metrics = androidx.window.layout.WindowMetricsCalculator.getOrCreate();
        return Build.VERSION.SDK_INT >= Build.VERSION_CODES.N && isInMultiWindowMode() &&
                !metrics.computeCurrentWindowMetrics(this).getBounds()
                .equals(metrics.computeMaximumWindowMetrics(this).getBounds());
    }

    @Override public void onConfigurationChanged(Configuration configuration) {
        super.onConfigurationChanged(configuration);
        androidx.core.view.ViewCompat.requestApplyInsets(findViewById(android.R.id.content));
        placeOverlay();
        controls.refreshLayout();
        if (showControls && (Build.VERSION.SDK_INT < Build.VERSION_CODES.N || !isInPictureInPictureMode())) controls.show();
        else controls.hide();
    }
}
