package com.limelight;

import com.limelight.binding.video.LatencyProbe;
import com.limelight.binding.video.PerformanceOverlay;
import com.limelight.nativeapi.NativeSession;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.http.ClipboardImageException;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.AppQuitOnLeave;
import com.limelight.utils.ClipboardImages;
import com.limelight.utils.Dialog;
import com.limelight.utils.ServerHelper;
import androidx.appcompat.app.AlertDialog;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.ui.NavigationDialogBuilder;
import android.content.Context;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Intent;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.os.PersistableBundle;
import android.view.KeyEvent;
import android.view.View;
import android.view.Window;
import android.widget.TextView;
import android.widget.Toast;
import org.xmlpull.v1.XmlPullParserException;
import java.io.IOException;
import com.limelight.ui.ActionSheet;
import java.util.Locale;

/** The stream menu and what it runs: host audio, the host's own menu, files and web UI, the microphone, stream health and latency tests, leaving or ending the session, USB devices, the clipboard, server commands and host status. */
final class GameMenuActions {
    private final Game game;

    GameMenuActions(Game game) {
        this.game = game;
    }

    // The sectioned stream menu while it is open; streamMenu is its sheet or a dialog it opened.
    private com.limelight.ui.StreamMenu streamMenuSheet;

    // A failed request may still have ended the session: the host can drop the stream before the
    // request fails, and the lost transport then takes up to its 10 s timeout to report. The
    // request has only failed once the stream is still connected after that.
    private static final long HOST_EXIT_CONFIRM_MS = 15000;
    private final Runnable hostExitFailed = () -> {
        if (GameMenuActions.this.game.connected && !GameMenuActions.this.game.hostActionInProgress) GameMenuActions.this.game.hostExitRequested = false;
    };

    void showStreamDialog(android.app.Dialog dialog) {
        game.keyboardInput.hidePcKeyboard();
        game.touchInput.setInputGrabState(false);
        game.suppressPipRefCount++;
        game.updatePipAutoEnter();
        game.streamMenu = dialog;
        dialog.setOnDismissListener(ignored -> {
            if (game.streamMenu == dialog) {
                game.streamMenu = null;
            }
            if (streamMenuSheet != null && streamMenuSheet.sheet() == dialog) {
                streamMenuSheet = null;
            }
            game.suppressPipRefCount--;
            game.updatePipAutoEnter();
            Runnable action = game.streamMenuAction;
            game.streamMenuAction = null;
            if (game.foreground && game.connected && !game.isFinishing()) {
                game.windowFocusActions.add(() -> {
                    game.touchInput.setInputGrabState(game.restoreInputAfterMenu);
                    game.hideSystemUi(1000);
                    if (action != null) {
                        action.run();
                    }
                }, SystemClock.uptimeMillis());
                View decorView = game.getWindow().getDecorView();
                decorView.postDelayed(game.runWindowFocusActions, WindowFocusActionQueue.TIMEOUT_MS);
                if (game.hasWindowFocus()) {
                    decorView.post(game.runWindowFocusActions);
                }
            }
        });
        ActionSheet.keepSystemBarsHidden(dialog, game);
        dialog.show();
    }

    /** The controller quit combo: leaves the stream like the menu's Disconnect. */
    void leaveStreamFromController() {
        if (!game.foreground || game.isFinishing()) {
            return;
        }
        leaveStream();
    }

    void showStreamMenu() {
        if (!game.foreground || !game.connected || game.streamMenu != null) {
            return;
        }
        game.restoreInputAfterMenu = game.grabbedInput;
        // Whoever opens the menu knows how; the first-stream hint has done its job.
        game.overlays.dismissStreamMenuHint(true);
        // The audio switches show the host's state as it is now, so they appear once it answers.
        hostAudio = null;
        hostAudioUnread = false;
        streamMenuSheet = new com.limelight.ui.StreamMenu(game, this::streamMenuOptions, this::onStreamMenuItem,
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
        return game.conn != null && game.app.getRole() == NvApp.Role.STREAM &&
                (game.conn.offersHostAudio(com.limelight.nativeapi.NativeCapabilities.NIGHT_MODE) ||
                        game.conn.offersHostAudio(com.limelight.nativeapi.NativeCapabilities.HEADPHONE_SURROUND));
    }

    // Reads the host's audio switches on the native menu thread; the open menu shows them once it answers.
    private void refreshHostAudio() {
        if (!offersHostAudio()) return;
        NvConnection connection = game.conn;
        int generation = game.foregroundGeneration;
        try {
            nativeMenuThread().execute(() -> {
                com.limelight.nativeapi.NativeAudio audio = null;
                try {
                    audio = connection.hostAudio();
                } catch (IOException e) {
                    LimeLog.warning("Host audio not read: " + e.getMessage());
                }
                com.limelight.nativeapi.NativeAudio answer = audio;
                game.runOnUiThread(() -> showHostAudio(answer, generation));
            });
        } catch (java.util.concurrent.RejectedExecutionException ignored) {
            // The stream is over.
        }
    }

    private void showHostAudio(com.limelight.nativeapi.NativeAudio audio, int generation) {
        if (generation != game.foregroundGeneration || game.isFinishing()) return;
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
        if (setting == null || !setting.switchable() || hostAudioBusy || game.conn == null) return;
        boolean on = !setting.value;
        NvConnection connection = game.conn;
        int generation = game.foregroundGeneration;
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
                game.runOnUiThread(() -> {
                    hostAudioBusy = false;
                    if (refused != null) {
                        LimeLog.warning("Host audio not changed: " + refused.getMessage());
                        if (game.foreground && generation == game.foregroundGeneration && game.connected && !game.isFinishing()) {
                            game.messages.alert(game.getString(R.string.stream_audio_failed, refused.getMessage()), Toast.LENGTH_LONG);
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
                                    game.getString(R.string.stream_audio_next_stream) : now.message;
                    if (on && note != null && game.foreground && generation == game.foregroundGeneration) {
                        game.messages.alert(note, Toast.LENGTH_LONG);
                    }
                });
            });
        } catch (java.util.concurrent.RejectedExecutionException ignored) {
            // The stream is over.
            hostAudioBusy = false;
        }
    }

    private com.limelight.console.NativeMenuController nativeMenuController;

    // The Radeon panel's reads and writes: one background thread, never the render, decode or input threads.
    private com.limelight.console.NativeMenuController nativeMenuController() {
        if (game.conn == null || !game.conn.hasNativeSessionActions() || game.app.getRole() != NvApp.Role.STREAM) {
            return null;
        }
        if (nativeMenuController == null) {
            java.util.concurrent.ScheduledExecutorService thread = nativeMenuThread();
            Handler main = new Handler(Looper.getMainLooper());
            nativeMenuController = new com.limelight.console.NativeMenuController(game.conn::nativeApiClient,
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
        if (game.nativeMenuThread == null) {
            game.nativeMenuThread = java.util.concurrent.Executors.newSingleThreadScheduledExecutor(task -> {
                Thread thread = new Thread(task, "Native menu");
                thread.setDaemon(true);
                return thread;
            });
        }
        return game.nativeMenuThread;
    }

    /**
     * Opens the menu the host draws into the stream. Like every menu action it runs once Nightjar's own sheet has
     * closed, so input reaches the stream (and the host's menu) again; the request runs on the native menu thread and
     * a refusal is shown with the host's message.
     */
    private void openHostMenu() {
        if (game.conn == null || !game.connected) return;
        NvConnection connection = game.conn;
        int generation = game.foregroundGeneration;
        try {
            nativeMenuThread().execute(() -> {
                try {
                    connection.openHostMenu();
                } catch (IOException e) {
                    LimeLog.warning("Host menu not opened: " + e.getMessage());
                    game.showHostActionError(e, generation);
                }
            });
        } catch (java.util.concurrent.RejectedExecutionException ignored) {
            // The stream is over.
        }
    }

    // What the stream menu offers right now. Host features and permissions decide the rest.
    private com.limelight.ui.StreamMenu.Options streamMenuOptions() {
        com.limelight.ui.StreamMenu.Options options = new com.limelight.ui.StreamMenu.Options(game.app.getRole(), game.conn.getHostDetails());
        options.canSendInput = game.conn.canSendInput();
        options.nativeHostControl = game.conn.hasNativeHostControl();
        options.hostMenu = game.conn.hasNativeCouchMenu();
        options.usbForwarding = game.prefConfig.usbForwarding && MoonBridge.isRustStream() && game.conn.offersUsbForwarding();
        options.imageClipboard = game.conn.hasImageClipboard();
        options.microphoneOffered = game.conn.getMicrophoneStatus().canMute();
        options.microphoneMuted = game.conn.isMicrophoneMuted();
        options.nightModeOffered = game.conn.offersHostAudio(com.limelight.nativeapi.NativeCapabilities.NIGHT_MODE);
        options.headphoneSurroundOffered = game.conn.offersHostAudio(com.limelight.nativeapi.NativeCapabilities.HEADPHONE_SURROUND);
        options.hostAudio = hostAudio;
        options.hostAudioUnread = hostAudioUnread;
        options.micButtonAvailable = game.micButton != null;
        options.micButtonShown = game.micButton != null && game.prefConfig.micButton;
        options.overlayShown = game.prefConfig.enablePerfOverlay;
        options.overlayMode = game.overlayMode;
        options.overlayPosition = game.overlayCorner.key;
        options.localOutput = game.externalPresentation == null;
        options.mouseConnected = game.touchInput.hasExternalMouse();
        options.controllerMouseAvailable = game.controllerHandler != null && game.controllerHandler.hasControllers();
        options.controllerMouseOn = game.controllerHandler != null && game.controllerHandler.isMouseEmulationActive();
        options.cursorVisible = game.cursorVisible;
        options.onscreenControls = game.prefConfig.onscreenController;
        options.gameControls = game.gameTouchControls != null;
        options.keyboardKeysShown = game.keyboardInput.isKeyboardOverlayVisible();
        options.scalingAvailable = game.canScaleLive();
        options.zoomEnabled = game.desktopGestures != null && game.desktopGestures.enabled();
        options.pictureInPicture = game.canEnterPictureInPicture();
        options.externalDisplays = game.hasPresentationDisplays();
        options.automaticQuality = game.supportsAdaptiveQuality();
        options.automaticBitrateOn = game.isAdaptiveBitrateEnabled() || game.isRateChangeBitrateOn();
        options.rateChange = game.rateChanges != null;
        options.rateChangeBitrate = game.rateChanges != null && game.prefConfig.rateChangeBitrate;
        options.webUiAddress = hostWebUiUrl() != null;
        options.connection = connectionInfo();
        return options;
    }

    // What the native page says about the stream; values the stream already holds, nothing read from the network.
    private com.limelight.console.ConnectionInfo connectionInfo() {
        if (game.decoderRenderer == null || game.prefConfig == null) return null;
        ComputerDetails details = game.conn.getHostDetails();
        return new com.limelight.console.ConnectionInfo(details.name, game.decoderRenderer.getActiveVideoFormat(),
                game.prefConfig.width, game.prefConfig.height, game.prefConfig.fps, game.currentBitrate == 0 ? game.prefConfig.bitrate : game.currentBitrate,
                game.wifiDiagnostics != null ? game.wifiDiagnostics.latest() : null,
                com.limelight.console.ConnectionInfo.rttMs(MoonBridge.getEstimatedRttInfo()));
    }

    private void onStreamMenuItem(com.limelight.ui.StreamMenu.Item item) {
        switch (item) {
            // The quick toggles change in place and leave the menu open.
            case OVERLAY: game.overlays.togglePerformanceOverlay(); return;
            case MICROPHONE: toggleMicrophoneMute(); return;
            case NIGHT_MODE: toggleHostAudio(com.limelight.nativeapi.NativeCapabilities.NIGHT_MODE); return;
            case HEADPHONE_SURROUND: toggleHostAudio(com.limelight.nativeapi.NativeCapabilities.HEADPHONE_SURROUND); return;
            default: break;
        }
        Runnable action = null;
        ComputerDetails details = game.conn.getHostDetails();
        switch (item) {
            case PC_KEYBOARD: action = game.keyboardInput::togglePcKeyboard; break;
            case KEYBOARD: action = game::toggleKeyboard; break;
            case TOUCH_MODE: action = game.touchInput::showTouchModeDialog; break;
            case MOUSE_CURSOR: action = game.touchInput::toggleCursorVisibility; break;
            case CONTROLLER_MOUSE: action = () -> { if (game.controllerHandler != null) game.controllerHandler.toggleMouseEmulationFromMenu(); }; break;
            case ONSCREEN_CONTROLS: action = () -> game.touchInput.setOnscreenControlsEnabled(!game.prefConfig.onscreenController); break;
            case CONTROLS_LAYOUT: action = game.touchInput::showControllerLayoutDialog; break;
            case MIC_BUTTON: action = () -> game.touchInput.setMicButtonShown(!game.prefConfig.micButton); break;
            case GAME_CONTROLS: action = game.gameTouchControls == null ? null : game.gameTouchControls::showMenu; break;
            case KEYBOARD_KEYS: action = game.keyboardInput::toggleKeyboardOverlay; break;
            case VIDEO_SCALING: action = game::showVideoScalingDialog; break;
            case ZOOM: action = game::toggleDesktopZoom; break;
            case ZOOM_RESET: action = () -> {
                game.desktopGestures.reset();
                if (game.trackpadPointer != null) game.trackpadPointer.syncCursor();
            }; break;
            case PICTURE_IN_PICTURE: action = game::enterPictureInPictureNow; break;
            case VIDEO_OUTPUT: action = game::showVideoOutputDialog; break;
            case BITRATE: action = game::showBitrateDialog; break;
            case RATE_CHANGE: action = game::showRateChangeDialog; break;
            case AUTOMATIC_QUALITY: action = game::showAdaptiveQualityDialog; break;
            case AUTOMATIC_BITRATE: action = game::toggleAdaptiveBitrateFromMenu; break;
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
            case RECONNECT: action = game::reconnectStream; break;
            case DISCONNECT: action = this::leaveStream; break;
            case QUIT_GAME: action = this::confirmQuitApp; break;
            case END_HOST_SESSION: action = this::confirmEndHostSession; break;
            case CLOSE_GAME: action = this::confirmCloseGame; break;
            case END_SESSION: action = this::endRemoteSession; break;
            case OVERLAY_OFF: action = game.overlays::hidePerformanceOverlay; break;
            case OVERLAY_TOP_LEFT: action = () -> game.overlays.setPerformanceOverlayPosition(com.limelight.ui.OverlayPlacement.Corner.TOP_LEFT); break;
            case OVERLAY_TOP_RIGHT: action = () -> game.overlays.setPerformanceOverlayPosition(com.limelight.ui.OverlayPlacement.Corner.TOP_RIGHT); break;
            case OVERLAY_BOTTOM_LEFT: action = () -> game.overlays.setPerformanceOverlayPosition(com.limelight.ui.OverlayPlacement.Corner.BOTTOM_LEFT); break;
            case OVERLAY_BOTTOM_RIGHT: action = () -> game.overlays.setPerformanceOverlayPosition(com.limelight.ui.OverlayPlacement.Corner.BOTTOM_RIGHT); break;
            case OVERLAY_COMPACT: action = () -> game.overlays.showPerformanceOverlay(PerformanceOverlay.MODE_COMPACT); break;
            case OVERLAY_STATISTICS: action = () -> game.overlays.showPerformanceOverlay(PerformanceOverlay.MODE_STATISTICS); break;
            case OVERLAY_DEBUG: action = () -> game.overlays.showPerformanceOverlay(PerformanceOverlay.MODE_DEBUG); break;
            case OVERLAY_COPY: action = game.overlays::copyPerformanceStats; break;
            case STREAM_HEALTH: action = this::showStreamHealth; break;
            case LATENCY_TEST: action = this::runLatencyTest; break;
            default: break;
        }
        // Like every menu action, it runs once the menu has closed and the stream has focus again.
        game.streamMenuAction = action;
        if (streamMenuSheet != null) streamMenuSheet.sheet().dismiss();
    }

    /**
     * A USB-driver controller produces no Android key events, so its D-pad, A and B reach the open
     * stream menu or dialog from here: B goes back, A chooses, the D-pad moves the focus.
     */
    void steerStreamMenu(int keyCode) {
        android.app.Dialog dialog = game.streamMenu;
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

    // The host web UI's address for this stream: the address and HTTPS port the stream connected to.
    private okhttp3.HttpUrl hostWebUiUrl() {
        ComputerDetails details = game.conn.getHostDetails();
        String address = game.getIntent().getStringExtra(Game.EXTRA_HOST);
        if (details == null || address == null) return null;
        ComputerDetails target = new ComputerDetails();
        target.state = ComputerDetails.State.ONLINE;
        target.caps = details.caps;
        target.activeAddress = new ComputerDetails.AddressTuple(address,
                game.getIntent().getIntExtra(Game.EXTRA_PORT, NvHTTP.DEFAULT_HTTP_PORT));
        target.httpsPort = details.httpsPort != 0 ? details.httpsPort : game.getIntent().getIntExtra(Game.EXTRA_HTTPS_PORT, 0);
        return ServerHelper.getHostSettingsUrl(target);
    }

    private void showHostWebUi() {
        okhttp3.HttpUrl url = hostWebUiUrl();
        if (url == null) return;
        AlertDialog dialog = new NavigationDialogBuilder(game).setTitle(R.string.host_administration_menu)
                .setMessage(game.getString(R.string.host_administration_help, url.toString()) + "\n\n" +
                        game.getString(R.string.stream_web_ui_leaves))
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.host_administration_open, (prompt, which) -> game.streamMenuAction = () -> {
                    try {
                        game.startActivity(new Intent(Intent.ACTION_VIEW, android.net.Uri.parse(url.toString())));
                    } catch (android.content.ActivityNotFoundException | SecurityException e) {
                        game.messages.alert(R.string.host_administration_no_browser, Toast.LENGTH_LONG);
                    }
                }).create();
        showStreamDialog(dialog);
        TextView message = dialog.findViewById(android.R.id.message);
        if (message != null) message.setTextIsSelectable(true);
    }

    /** The PC's file transfer screen; it opens over the stream, so the menu says it leaves it first. */
    private void showFiles() {
        String uuid = game.getIntent().getStringExtra(Game.EXTRA_PC_UUID);
        if (uuid == null) return;
        showStreamDialog(new NavigationDialogBuilder(game).setTitle(R.string.files_menu)
                .setMessage(R.string.files_stream_leaves)
                .setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.files_open, (prompt, which) -> game.streamMenuAction = () ->
                        game.startActivity(com.limelight.files.FileTransferActivity.forPc(game, uuid)))
                .create());
    }

    private final com.limelight.binding.audio.MicNotice.Once microphoneNotice = new com.limelight.binding.audio.MicNotice.Once();

    // The microphone settings ask for processing this device lacks: say so in the stream, once per difference.
    // A microphone that failed and was muted is said every time, since each unmute tries again.
    void showMicrophoneNotice() {
        if (game.conn == null || game.isFinishing() || game.isDestroyed()) return;
        if (game.conn.takeMicrophoneFailure()) {
            game.messages.alert(game.getString(R.string.mic_failed), Toast.LENGTH_LONG);
            if (game.micButton != null) game.micButton.refresh();
        }
        String text = microphoneNotice.next(com.limelight.binding.audio.MicNotice.text(game, game.conn.takeMicrophoneNotice()));
        if (text != null) game.messages.alert(text, Toast.LENGTH_LONG);
    }

    private void toggleMicrophoneMute() {
        if (game.conn == null) return;
        game.conn.setMicrophoneMuted(!game.conn.isMicrophoneMuted());
        if (game.micButton != null) game.micButton.refresh();
    }

    private boolean latencyTestRunning;

    private void showStreamHealth() {
        com.limelight.utils.StreamHealth.Report report = game.streamHealth.report(SystemClock.uptimeMillis());
        StringBuilder text = Game.healthText(game, report, wifiText(0));
        boolean manual = Game.healthTrialAllowed(game.isAdaptiveBitrateEnabled(), game.isRateChangeBitrateOn(), game.adaptiveQuality != null);
        boolean undo = game.healthOriginalBitrate > 0 && game.currentBitrate == game.healthTrialBitrate && manual;
        boolean suggest = report.canSuggestBitrate() && game.supportsAdaptiveBitrate() && manual &&
                game.currentBitrate > 500 && !game.hostActionInProgress && !undo;
        if (undo || suggest) text.append("\n\n").append(game.getString(R.string.health_trial_help));
        MaterialAlertDialogBuilder builder = new NavigationDialogBuilder(game).setTitle(R.string.health_title)
                .setMessage(text).setNegativeButton(android.R.string.cancel, null)
                .setNeutralButton(R.string.overlay_copy, (dialog, which) -> {
                    ClipboardManager clipboard = (ClipboardManager) game.getSystemService(Context.CLIPBOARD_SERVICE);
                    clipboard.setPrimaryClip(ClipData.newPlainText(game.getString(R.string.health_title), text));
                });
        if (undo || suggest) builder.setPositiveButton(undo ? R.string.health_undo : R.string.health_try,
                (dialog, which) -> game.applyHealthBitrate(undo));
        else builder.setPositiveButton(android.R.string.ok, null);
        showStreamDialog(builder.create());
    }

    String wifiText(long stallPeriodMs) {
        com.limelight.utils.WifiDiagnostics.Lock lock = Build.VERSION.SDK_INT < Build.VERSION_CODES.Q ?
                com.limelight.utils.WifiDiagnostics.Lock.UNSUPPORTED :
                game.lowLatencyWifiLock != null && game.lowLatencyWifiLock.isHeld() ?
                        com.limelight.utils.WifiDiagnostics.Lock.HELD : com.limelight.utils.WifiDiagnostics.Lock.NOT_HELD;
        return com.limelight.utils.WifiDiagnostics.describe(game,
                game.wifiDiagnostics != null ? game.wifiDiagnostics.latest() : null, lock, stallPeriodMs);
    }

    // Nudges the host mouse back and forth on a still screen and times each nudge until its frame is shown.
    private void runLatencyTest() {
        if (latencyTestRunning || !game.connected) return;
        latencyTestRunning = true;
        game.messages.alert(R.string.latency_test_running, Toast.LENGTH_LONG);
        LatencyProbe probe = game.decoderRenderer.getLatencyProbe();
        NvConnection connection = game.conn;
        new Thread(() -> {
            final int probes = 15;
            long[] samples = new long[probes];
            int count = 0;
            short step = 8;
            long giveUpNs = System.nanoTime() + 20_000_000_000L;
            try {
                while (count < probes && game.connected && System.nanoTime() < giveUpNs) {
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
            game.runOnUiThread(() -> {
                latencyTestRunning = false;
                if (!game.foreground || !game.connected || game.isFinishing()) return;
                String message = result == null || measured < 5 ? game.getString(R.string.latency_test_failed) :
                        game.getString(R.string.latency_test_result, result[1], result[0], result[2], measured);
                LimeLog.info("Latency test: " + message);
                showStreamDialog(new NavigationDialogBuilder(game).setTitle(R.string.latency_test)
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
        androidx.appcompat.app.AlertDialog dialog = new NavigationDialogBuilder(game).setTitle(title)
                .setMessage(message).setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(title, (prompt, which) -> game.streamMenuAction = exit).create();
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
        runHostExit("Quit stream app", () -> game.conn.closeGame());
    }

    /** Ends the stream session on the PC and leaves; the game keeps running. */
    private void endHostSession() {
        runHostExit("End host session", () -> game.conn.nativeSessionAction(NativeSession.ActionId.END));
    }

    private interface HostExit {
        void run() throws IOException, XmlPullParserException;
    }

    private void runHostExit(String threadName, HostExit exit) {
        if (game.hostActionInProgress || !game.foreground || !game.connected) return;
        int generation = game.foregroundGeneration;
        game.hostActionInProgress = true;
        game.hostExitRequested = true;
        new Thread(() -> {
            try {
                exit.run();
                game.runOnUiThread(game::finish);
            } catch (IOException | XmlPullParserException e) {
                game.runOnUiThread(this::confirmHostExitFailedLater);
                game.showHostActionError(e, generation);
            } finally {
                game.runOnUiThread(() -> game.hostActionInProgress = false);
            }
        }, threadName).start();
    }

    /**
     * Leaves the stream as the player asked. A game set to quit on leave is quit first, while the
     * stream still holds the host connection; a failure is reported because the game stays running.
     */
    void leaveStream() {
        boolean chosen = AppQuitOnLeave.isEnabled(game, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID),
                game.app.getAppUuid(), game.app.getAppId());
        // A plain disconnect never waits for a host request; its callbacks check the activity.
        if (!StreamExitPolicy.quitsOnLeave(chosen, game.app.getRole(), game.connected)) {
            if (game.connected && game.conn != null) game.conn.announceDisconnect();
            game.finish();
            return;
        }
        if (game.hostActionInProgress) {
            game.messages.alert(R.string.stream_bitrate_busy, Toast.LENGTH_SHORT);
            return;
        }
        String name = game.appName != null ? game.appName : game.app.getAppName();
        Context context = game.getApplicationContext();
        game.hostActionInProgress = true;
        game.hostExitRequested = true;
        Toast.makeText(context, game.getString(R.string.stream_quit_on_leave_progress, name), Toast.LENGTH_SHORT).show();
        new Thread(() -> {
            Exception failure = null;
            try {
                game.conn.closeGame();
            } catch (IOException | XmlPullParserException e) {
                failure = e;
            }
            Exception error = failure;
            game.runOnUiThread(() -> {
                game.hostActionInProgress = false;
                // A host that ends the stream while it quits cancels the reply, so a failure counts
                // only while the stream is still up.
                if (error != null && game.connected) {
                    LimeLog.warning("Quit on leave failed: " + error.getMessage());
                    Toast.makeText(context, context.getString(R.string.stream_quit_on_leave_failed, name,
                            error.getMessage()), Toast.LENGTH_LONG).show();
                }
                game.finish();
            });
        }, "Quit on leave").start();
    }

    private void endRemoteSession() {
        if (game.hostActionInProgress) {
            return;
        }
        final int generation = game.foregroundGeneration;
        game.hostActionInProgress = true;
        game.hostExitRequested = true;
        new Thread(() -> {
            try {
                game.conn.disconnectRemoteSession();
                game.runOnUiThread(game::finish);
            } catch (IOException | XmlPullParserException e) {
                game.runOnUiThread(this::confirmHostExitFailedLater);
                game.showHostActionError(e, generation);
            } finally {
                game.runOnUiThread(() -> game.hostActionInProgress = false);
            }
        }, "End remote session").start();
    }

    private void confirmHostExitFailedLater() {
        game.bitrateHandler.removeCallbacks(hostExitFailed);
        game.bitrateHandler.postDelayed(hostExitFailed, HOST_EXIT_CONFIRM_MS);
    }

    private void refreshHostStatus() {
        if (game.hostActionInProgress) {
            return;
        }
        final int generation = game.foregroundGeneration;
        game.hostActionInProgress = true;
        new Thread(() -> {
            try {
                ComputerDetails details = game.conn.refreshHostDetails();
                game.runOnUiThread(() -> {
                    if (!game.foreground || generation != game.foregroundGeneration || !game.connected || game.isFinishing() || game.streamMenu != null) {
                        return;
                    }
                    showHostStatus(details);
                });
            } catch (IOException | XmlPullParserException e) {
                game.showHostActionError(e, generation);
            } finally {
                game.runOnUiThread(() -> game.hostActionInProgress = false);
            }
        }, "Host status").start();
    }

    // The devices plugged into this one that may go to the PC; picking a forwarded one detaches it.
    private void showUsbDevices() {
        if (!game.connected) return;
        if (game.usbForwarding == null) {
            game.usbForwarding = new com.limelight.binding.usb.UsbForwarding(game, game.prefConfig, game.conn,
                    text -> game.messages.alert(text, Toast.LENGTH_LONG));
        }
        com.limelight.binding.usb.UsbForwarding forwarding = game.usbForwarding;
        java.util.List<android.hardware.usb.UsbDevice> devices = forwarding.candidates();
        com.google.android.material.dialog.MaterialAlertDialogBuilder builder = new NavigationDialogBuilder(game)
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
        if (game.hostActionInProgress || !game.foreground || !game.connected) {
            return;
        }
        ClipboardManager clipboard = (ClipboardManager) game.getSystemService(Context.CLIPBOARD_SERVICE);
        String text = null;
        if (send) {
            final ClipData clip;
            try {
                clip = clipboard.getPrimaryClip();
            } catch (RuntimeException e) {
                game.showHostActionError(e, game.foregroundGeneration);
                return;
            }
            if (clip == null || clip.getItemCount() == 0 || clip.getItemAt(0).getText() == null) {
                game.messages.alert(R.string.stream_clipboard_empty, Toast.LENGTH_SHORT);
                return;
            }
            text = clip.getItemAt(0).getText().toString();
        }
        final String outgoing = text;
        final int generation = game.foregroundGeneration;
        game.hostActionInProgress = true;
        new Thread(() -> {
            try {
                if (send) {
                    game.conn.sendClipboard(outgoing);
                }
                String incoming = send ? null : game.conn.getClipboard();
                game.runOnUiThread(() -> {
                    // Window focus is not asked: a toast or the stream menu's dialog holds it while the stream stays.
                    if (!game.foreground || generation != game.foregroundGeneration || !game.connected || game.isFinishing()) {
                        if (!send) {
                            game.messages.alert(R.string.stream_clipboard_not_copied, Toast.LENGTH_LONG);
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
                            game.showHostActionError(e, generation);
                            return;
                        }
                    }
                    game.messages.alert(send ? R.string.stream_clipboard_sent : R.string.stream_clipboard_received, Toast.LENGTH_SHORT);
                });
            } catch (IOException | XmlPullParserException e) {
                game.showHostActionError(e, generation);
            } finally {
                game.runOnUiThread(() -> game.hostActionInProgress = false);
            }
        }, "Clipboard transfer").start();
    }

    /**
     * Sends the image on this device's clipboard to the host as a PNG, or puts the host's PNG on this device's
     * clipboard as a content URI. Android 13 and later confirm a copy themselves, so only older ones get a toast.
     */
    private void transferClipboardImage(boolean send) {
        if (game.hostActionInProgress || !game.foreground || !game.connected) {
            return;
        }
        ClipboardManager clipboard = (ClipboardManager) game.getSystemService(Context.CLIPBOARD_SERVICE);
        ClipData current = null;
        if (send) {
            try {
                current = clipboard.getPrimaryClip();
            } catch (RuntimeException e) {
                game.showHostActionError(e, game.foregroundGeneration);
                return;
            }
            if (current == null || current.getItemCount() == 0) {
                game.messages.alert(R.string.stream_clipboard_image_empty, Toast.LENGTH_SHORT);
                return;
            }
        }
        final ClipData outgoing = current;
        final int generation = game.foregroundGeneration;
        game.hostActionInProgress = true;
        new Thread(() -> {
            java.io.File staged = null;
            try {
                if (send) {
                    // The image's type comes from its provider, which may block: never asked on the main thread.
                    android.net.Uri image = ClipboardImages.imageUri(outgoing, game.getContentResolver());
                    if (image == null) {
                        showClipboardImageResult(R.string.stream_clipboard_image_empty, generation);
                        return;
                    }
                    staged = ClipboardImages.readPng(game, image);
                    game.conn.sendClipboardImage(staged);
                    showClipboardImageResult(R.string.stream_clipboard_image_sent, generation);
                    return;
                }
                staged = ClipboardImages.newIncoming(game);
                boolean found;
                try (java.io.OutputStream out = new java.io.FileOutputStream(staged)) {
                    found = game.conn.getClipboardImage(out);
                }
                if (!found) {
                    showClipboardImageResult(R.string.stream_clipboard_image_none, generation);
                    return;
                }
                java.io.File incoming = staged;
                staged = null;
                game.runOnUiThread(() -> placeClipboardImage(clipboard, incoming, generation));
            } catch (ClipboardImageException e) {
                LimeLog.warning("Clipboard image refused: " + e.getMessage());
                showClipboardImageResult(ClipboardImages.message(e.reason, send), generation);
            } catch (OutOfMemoryError e) {
                LimeLog.warning("Clipboard image out of memory: " + e);
                showClipboardImageResult(ClipboardImages.message(ClipboardImageException.Reason.TOO_LARGE, send),
                        generation);
            } catch (IOException | XmlPullParserException | RuntimeException e) {
                game.showHostActionError(e, generation);
            } finally {
                if (staged != null) {
                    ClipboardImages.discard(staged);
                }
                game.runOnUiThread(() -> game.hostActionInProgress = false);
            }
        }, "Clipboard image transfer").start();
    }

    /**
     * Puts the host's image, streamed to {@code image}, on this device's clipboard; the images before it go only once
     * it is there. A stream that closed meanwhile gets it said, not a silent drop.
     */
    private void placeClipboardImage(ClipboardManager clipboard, java.io.File image, int generation) {
        // Window focus is not asked: a toast or the stream menu's dialog holds it while the stream stays.
        if (!game.foreground || generation != game.foregroundGeneration || !game.connected || game.isFinishing()) {
            ClipboardImages.discard(image);
            game.messages.alert(R.string.stream_clipboard_image_not_copied, Toast.LENGTH_LONG);
            return;
        }
        try {
            ClipData clip = ClipData.newUri(game.getContentResolver(), BuildConfig.BRAND_NAME, ClipboardImages.share(game, image));
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                PersistableBundle extras = new PersistableBundle();
                extras.putBoolean("android.content.extra.IS_SENSITIVE", true);
                clip.getDescription().setExtras(extras);
            }
            clipboard.setPrimaryClip(clip);
        } catch (RuntimeException e) {
            ClipboardImages.discard(image);
            game.showHostActionError(e, generation);
            return;
        }
        ClipboardImages.keepOnly(game, image);
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            game.messages.alert(R.string.stream_clipboard_image_received, Toast.LENGTH_SHORT);
        }
    }

    private void showClipboardImageResult(int message, int generation) {
        game.runOnUiThread(() -> {
            if (game.foreground && generation == game.foregroundGeneration && game.connected && !game.isFinishing()) {
                game.messages.alert(message, Toast.LENGTH_LONG);
            }
        });
    }

    private void showServerCommands(ComputerDetails details) {
        String[] names = details.serverCommands.subList(0, Math.min(256, details.serverCommands.size())).toArray(new String[0]);
        showStreamDialog(new NavigationDialogBuilder(game).setTitle(R.string.stream_server_commands)
                .setItems(names, (dialog, index) -> {
                    dialog.dismiss();
                    showStreamDialog(new NavigationDialogBuilder(game).setTitle(names[index])
                            .setMessage(R.string.stream_command_confirm)
                            .setNegativeButton(android.R.string.cancel, null)
                            .setPositiveButton(R.string.stream_command_run, (confirmation, which) -> {
                                if (!game.foreground || !game.connected || game.hostActionInProgress) {
                                    return;
                                }
                                game.hostActionInProgress = true;
                                final int generation = game.foregroundGeneration;
                                new Thread(() -> {
                                    try {
                                        boolean sent = game.conn.sendServerCommand(index, names[index]);
                                        game.runOnUiThread(() -> {
                                            if (game.foreground && generation == game.foregroundGeneration && game.connected && !game.isFinishing()) {
                                                game.messages.alert(sent ? R.string.stream_command_sent : R.string.stream_command_failed, Toast.LENGTH_LONG);
                                            }
                                        });
                                    } catch (IOException | XmlPullParserException e) {
                                        game.showHostActionError(e, generation);
                                    } finally {
                                        game.runOnUiThread(() -> game.hostActionInProgress = false);
                                    }
                                }, "Server command").start();
                            }).create());
                }).setNegativeButton(android.R.string.cancel, null).create());
    }

    private void showHostStatus(ComputerDetails details) {
        StringBuilder status = new StringBuilder();
        if (details.permission == -1) {
            status.append(game.getString(R.string.stream_permissions_unknown));
        } else {
            String[] names = game.getResources().getStringArray(R.array.host_permission_names);
            int[] masks = ComputerDetails.PERMISSION_DISPLAY_MASKS;
            for (int i = 0; i < names.length; i++) {
                status.append(names[i]).append(": ").append(game.getString(details.hasPermission(masks[i]) ?
                        R.string.stream_permission_allowed : R.string.stream_permission_denied)).append('\n');
            }
        }
        if (details.permission != -1) {
            status.append("\n\n").append(game.getString(R.string.stream_permissions_help));
        }
        if (details.caps.has(HostCapabilities.Feature.FRAME_LIMITER_STATUS)) {
            status.append("\n\n").append(game.getString(R.string.stream_limiter_status,
                    game.getString(details.frameLimiterEnabled ? R.string.stream_enabled : R.string.stream_disabled),
                    game.getString(details.virtualDisplayFrameLimiterEnabled ? R.string.stream_enabled : R.string.stream_disabled),
                    details.frameLimiterFpsLimitMilliHz == 0 ? game.getString(R.string.stream_limiter_stream_rate) :
                            String.format(Locale.getDefault(), "%.3f FPS", details.frameLimiterFpsLimitMilliHz / 1000.0)));
        }
        status.append("\n\n").append(game.getString(!game.prefConfig.vrr ? R.string.stream_android_pacing :
                game.useArr ? R.string.stream_android_arr : R.string.stream_android_max_refresh));
        showStreamDialog(new NavigationDialogBuilder(game).setTitle(R.string.stream_host_status)
                .setMessage(status).setPositiveButton(android.R.string.ok, null).create());
    }
}
