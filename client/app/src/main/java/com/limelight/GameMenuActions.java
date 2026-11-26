package com.limelight;

import android.content.Context;
import android.os.Build;
import android.os.SystemClock;
import android.view.KeyEvent;
import android.view.View;
import android.widget.Toast;

import androidx.appcompat.app.AlertDialog;

import com.limelight.binding.video.PerformanceOverlay;
import com.limelight.nativeapi.NativeSession;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.AppQuitOnLeave;
import com.limelight.ui.ActionSheet;
import com.limelight.ui.NavigationDialogBuilder;

import org.xmlpull.v1.XmlPullParserException;

import java.io.IOException;

/** The stream menu and what it runs: sheet lifecycle, options, controller navigation, leaving or ending the session, and server commands; host-side actions delegate to {@link GameHostActions}. */
final class GameMenuActions {
    private final Game game;
    private final GameHostActions hostActions;

    GameMenuActions(Game game) {
        this.game = game;
        this.hostActions = new GameHostActions(game, this);
    }

    // The sectioned stream menu while it is open; streamMenu is its sheet or a dialog it opened.
    private com.limelight.ui.StreamMenu streamMenuSheet;
    // Whether the system keyboard was open as the stream menu opened.
    private boolean systemKeyboardAtMenu;
    // Whether the current streamMenu is a post-stream / termination prompt that must survive onPause().
    private boolean streamMenuTerminal;

    // A failed request may still have ended the session: the host can drop the stream before the
    // request fails, and the lost transport then takes up to its 10 s timeout to report. The
    // request has only failed once the stream is still connected after that.
    private static final long HOST_EXIT_CONFIRM_MS = 15000;
    private final Runnable hostExitFailed = () -> {
        if (!GameMenuActions.this.game.connected) return;
        if (GameMenuActions.this.game.hostActionInProgress) {
            confirmHostExitFailedLater();
            return;
        }
        GameMenuActions.this.game.hostExitRequested = false;
    };

    boolean isTerminalDialog() {
        return streamMenuTerminal;
    }

    void showStreamDialog(android.app.Dialog dialog) {
        showStreamDialog(dialog, false);
    }

    void showTerminalDialog(android.app.Dialog dialog) {
        showStreamDialog(dialog, true);
    }

    private void showStreamDialog(android.app.Dialog dialog, boolean terminal) {
        if (game.streamMenu == dialog) {
            streamMenuTerminal = terminal;
            return;
        }
        game.keyboardInput.hidePcKeyboard();
        game.touchInput.setInputGrabState(false);
        android.app.Dialog previous = game.streamMenu;
        game.streamMenuAction = null;
        streamMenuTerminal = terminal;
        game.streamMenu = dialog;
        game.suppressPipRefCount++;
        game.videoOutput.updatePipAutoEnter();
        if (previous != null) {
            previous.dismiss();
        }
        dialog.setOnDismissListener(ignored -> {
            boolean wasCurrent = game.streamMenu == dialog;
            if (wasCurrent) {
                game.streamMenu = null;
                streamMenuTerminal = false;
            }
            if (streamMenuSheet != null && streamMenuSheet.sheet() == dialog) {
                streamMenuSheet = null;
            }
            if (game.streamConnection != null && game.streamConnection.moveWatchDialog == dialog) {
                game.streamConnection.destroyMoveWatch();
            }
            game.suppressPipRefCount--;
            game.videoOutput.updatePipAutoEnter();
            if (!wasCurrent) {
                return;
            }
            Runnable action = game.streamMenuAction;
            game.streamMenuAction = null;
            if (game.foreground && game.connected && !game.isFinishing()) {
                game.windowFocusActions.add(() -> {
                    game.touchInput.setInputGrabState(game.restoreInputAfterMenu);
                    game.videoOutput.hideSystemUi(1000);
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
        com.limelight.ui.StreamGlass.show(dialog);
    }

    /** The stream menu is open: its bitrate slider already shows an applied rate, and a toast would cover its ways out. */
    boolean streamMenuOpen() {
        return streamMenuSheet != null && streamMenuSheet.sheet().isShowing();
    }

    /**
     * The stream's bitrate for an open menu's slider: disabled while the PC has not said what it counts it as, usable
     * at the stream's number once it did (or the wait ended without it).
     */
    void showBitrate(int kbps, boolean usable) {
        if (streamMenuSheet != null) streamMenuSheet.showBitrate(kbps, usable);
    }

    /** The controller quit combo: leaves the stream like the menu's Disconnect. */
    void leaveStreamFromController() {
        if (!game.foreground || game.isFinishing()) {
            return;
        }
        leaveStream();
    }

    // The user's shortcuts in the stream menu: kept with the app's other shortcuts, pressed on the PC once the
    // menu has closed and the stream has focus again, like every menu action.
    private final com.limelight.ui.StreamMenu.Shortcuts menuShortcuts = new com.limelight.ui.StreamMenu.Shortcuts() {
        @Override public java.util.List<com.limelight.binding.input.KeyboardShortcuts.Shortcut> saved() {
            return com.limelight.binding.input.KeyboardShortcuts.load(game);
        }

        @Override public void save(java.util.List<com.limelight.binding.input.KeyboardShortcuts.Shortcut> shortcuts) {
            com.limelight.binding.input.KeyboardShortcuts.save(game, shortcuts);
        }

        @Override public void send(int[] keys) {
            game.streamMenuAction = () -> game.keyboardInput.sendMenuShortcut(keys);
            if (streamMenuSheet != null) streamMenuSheet.sheet().dismiss();
        }
    };

    void showStreamMenu(boolean byController) {
        if (!game.foreground || !game.connected || game.streamMenu != null) {
            return;
        }
        game.restoreInputAfterMenu = game.grabbedInput;
        // The menu takes the system keyboard away for as long as it is open; its Keyboard tile then closes it.
        systemKeyboardAtMenu = game.keyboardInput.isSystemKeyboardVisible();
        // Whoever opens the menu knows how; the first-stream hint has done its job.
        game.overlays.dismissStreamMenuHint(true);
        // The audio switches show the host's state as it is now, so they appear once it answers.
        hostActions.resetHostAudio();
        streamMenuSheet = new com.limelight.ui.StreamMenu(game, this::streamMenuOptions, this::onStreamMenuItem,
                hostActions.nativeMenuController(), menuShortcuts);
        showStreamDialog(streamMenuSheet.sheet());
        if (byController) streamMenuSheet.sheet().focusFirstActionFromController();
        hostActions.refreshHostAudio();
    }

    // The stream's size or rate changed: a menu left open names the new ones in its header. UI thread.
    void refreshStreamMenu() {
        if (streamMenuSheet != null) streamMenuSheet.refresh();
    }

    // What the stream menu offers right now. Host features and permissions decide the rest.
    private com.limelight.ui.StreamMenu.Options streamMenuOptions() {
        com.limelight.ui.StreamMenu.Options options = new com.limelight.ui.StreamMenu.Options(game.app.getRole(), game.conn.getHostDetails());
        options.appName = game.app.getAppName();
        options.cover = game.streamConnection.connectingCover();
        options.bitrateKbps = game.currentBitrate == 0 ? game.prefConfig.bitrate : game.currentBitrate;
        options.bitrateChange = game.adaptation::applyBitrateFromMenu;
        options.bitrateWaiting = game.adaptation.bitrateWaiting();
        options.canSendInput = game.conn.canSendInput();
        options.nativeHostControl = game.conn.hasNativeHostControl();
        options.hostMenu = game.conn.hasNativeCouchMenu();
        options.usbForwarding = game.prefConfig.usbForwarding && game.conn.offersUsbForwarding();
        options.imageClipboard = game.conn.hasImageClipboard();
        options.sessionGuests = game.streamGuests != null && game.streamGuests.manages();
        options.microphoneOffered = game.conn.getMicrophoneStatus().canMute();
        options.microphoneMuted = game.conn.isMicrophoneMuted();
        options.nightModeOffered = game.conn.offersHostAudio(com.limelight.nativeapi.NativeCapabilities.NIGHT_MODE);
        options.headphoneSurroundOffered = game.conn.offersHostAudio(com.limelight.nativeapi.NativeCapabilities.HEADPHONE_SURROUND);
        options.hostAudio = hostActions.hostAudio();
        options.hostAudioUnread = hostActions.hostAudioUnread();
        options.micButtonAvailable = game.micButton != null;
        options.micButtonShown = game.micButton != null && game.prefConfig.micButton;
        options.menuButtonAvailable = game.menuButton != null;
        options.menuButtonShown = game.menuButton != null && game.prefConfig.floatingMenuButton;
        // The size the decoder was set up for or the host changed to, not the size asked for: the host may lower it
        // (4K to a host without 4K), choose it (native) or change it mid-stream.
        options.streamWidth = game.streamWidth;
        options.streamHeight = game.streamHeight;
        options.streamFps = game.streamFps;
        options.videoFormat = game.decoderRenderer == null ? 0 : game.decoderRenderer.getActiveVideoFormat();
        options.hdr = game.streamHdr;
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
        options.scalingAvailable = game.videoOutput.canScaleLive();
        options.picturePositionAvailable = game.safeArea.applies();
        options.flipAvailable = game.safeArea.canFlip();
        options.zoomEnabled = game.desktopGestures != null && game.desktopGestures.enabled();
        options.zoomed = game.desktopGestures != null && game.desktopGestures.viewport().scale() > 1.01f;
        options.pictureInPicture = game.videoOutput.canEnterPictureInPicture();
        options.externalDisplays = game.videoOutput.hasPresentationDisplays();
        options.automaticQuality = game.adaptation.showsAdaptiveQuality();
        options.automaticBitrateOn = game.adaptation.isAdaptiveBitrateEnabled() || game.adaptation.isRateChangeBitrateOn();
        options.rateChange = game.rateChanges != null;
        options.rateChangeBitrate = game.rateChanges != null && game.prefConfig.rateChangeBitrate;
        options.webUiAddress = hostActions.hostWebUiUrl() != null;
        options.connection = connectionInfo();
        return options;
    }

    // What the native page says about the stream; values the stream already holds, nothing read from the network.
    private com.limelight.console.ConnectionInfo connectionInfo() {
        if (game.decoderRenderer == null || game.prefConfig == null) return null;
        ComputerDetails details = game.conn.getHostDetails();
        return new com.limelight.console.ConnectionInfo(details.name, game.decoderRenderer.getActiveVideoFormat(),
                game.streamWidth, game.streamHeight, game.streamFps, game.currentBitrate == 0 ? game.prefConfig.bitrate : game.currentBitrate,
                game.wifiDiagnostics != null ? game.wifiDiagnostics.latest() : null,
                com.limelight.console.ConnectionInfo.rttMs(MoonBridge.getEstimatedRttInfo()));
    }

    private void onStreamMenuItem(com.limelight.ui.StreamMenu.Item item) {
        switch (item) {
            // The quick toggles change in place and leave the menu open.
            case OVERLAY: game.overlays.togglePerformanceOverlay(); return;
            case MICROPHONE: hostActions.toggleMicrophoneMute(); return;
            case NIGHT_MODE: hostActions.toggleHostAudio(com.limelight.nativeapi.NativeCapabilities.NIGHT_MODE); return;
            case HEADPHONE_SURROUND: hostActions.toggleHostAudio(com.limelight.nativeapi.NativeCapabilities.HEADPHONE_SURROUND); return;
            default: break;
        }
        Runnable action = null;
        ComputerDetails details = game.conn.getHostDetails();
        switch (item) {
            case PC_KEYBOARD: {
                boolean open = systemKeyboardAtMenu;
                action = () -> game.keyboardInput.togglePcKeyboard(open);
                break;
            }
            case KEYBOARD: {
                boolean open = systemKeyboardAtMenu;
                action = () -> game.keyboardInput.toggleKeyboard(open);
                break;
            }
            case TOUCH_MODE: action = game.touchInput::showTouchModeDialog; break;
            case MOUSE_CURSOR: action = game.touchInput::toggleCursorVisibility; break;
            case CONTROLLER_MOUSE: action = () -> { if (game.controllerHandler != null) game.controllerHandler.toggleMouseEmulationFromMenu(); }; break;
            case ONSCREEN_CONTROLS: action = () -> game.touchInput.setOnscreenControlsEnabled(!game.prefConfig.onscreenController); break;
            case CONTROLS_LAYOUT: action = game.touchInput::showControllerLayoutDialog; break;
            case MIC_BUTTON: action = () -> game.touchInput.setMicButtonShown(!game.prefConfig.micButton); break;
            case MENU_BUTTON: action = () -> game.touchInput.setMenuButtonShown(!game.prefConfig.floatingMenuButton); break;
            case GAME_CONTROLS: action = game.gameTouchControls == null ? null : game.gameTouchControls::showMenu; break;
            case KEYBOARD_KEYS: action = game.keyboardInput::toggleKeyboardOverlay; break;
            case VIDEO_SCALING: action = game.videoOutput::showVideoScalingDialog; break;
            case PICTURE_POSITION: action = () -> showStreamDialog(game.safeArea.createPositionDialog()); break;
            case FLIP_SCREEN: action = () -> game.safeArea.flip(game.externalPresentation != null); break;
            case ZOOM: action = game.videoOutput::toggleDesktopZoom; break;
            case ZOOM_RESET: action = () -> {
                game.desktopGestures.reset();
                if (game.trackpadPointer != null) game.trackpadPointer.syncCursor();
            }; break;
            case PICTURE_IN_PICTURE: action = game.videoOutput::enterPictureInPictureNow; break;
            case VIDEO_OUTPUT: action = game.videoOutput::showVideoOutputDialog; break;
            case BITRATE: action = game.adaptation::showBitrateDialog; break;
            case RATE_CHANGE: action = game.adaptation::showRateChangeDialog; break;
            case AUTOMATIC_QUALITY: action = game.adaptation::showAdaptiveQualityDialog; break;
            case AUTOMATIC_BITRATE: action = game.adaptation::toggleAdaptiveBitrateFromMenu; break;
            case SERVER_COMMANDS: action = () -> showServerCommands(details); break;
            case CLIPBOARD_SEND: action = () -> transferClipboard(true); break;
            case CLIPBOARD_RECEIVE: action = () -> transferClipboard(false); break;
            case USB_DEVICES: action = hostActions::showUsbDevices; break;
            case GUESTS: action = () -> { if (game.streamGuests != null) game.streamGuests.showGuests(); }; break;
            case IMAGE_CLIPBOARD_SEND: action = () -> transferClipboardImage(true); break;
            case IMAGE_CLIPBOARD_RECEIVE: action = () -> transferClipboardImage(false); break;
            case FILES: action = hostActions::showFiles; break;
            case HOST_WEB_UI: action = hostActions::showHostWebUi; break;
            case HOST_STATUS: action = hostActions::refreshHostStatus; break;
            case HOST_MENU: action = hostActions::openHostMenu; break;
            case RECONNECT: action = game.streamRecovery::reconnectStream; break;
            case DISCONNECT: action = this::leaveStream; break;
            case CONFIRM_DISCONNECT: action = this::confirmDisconnect; break;
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
            case STREAM_HEALTH: action = hostActions::showStreamHealth; break;
            case LATENCY_TEST: action = hostActions::runLatencyTest; break;
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
        // Y, X, LB and RB run the tiles that show them, wherever the focus is.
        if (streamMenuSheet != null && dialog == streamMenuSheet.sheet()) {
            if (streamMenuSheet.hotkey(keyCode)) return;
        }
        if (keyCode == KeyEvent.KEYCODE_BUTTON_X || keyCode == KeyEvent.KEYCODE_BUTTON_Y ||
                keyCode == KeyEvent.KEYCODE_BUTTON_L1 || keyCode == KeyEvent.KEYCODE_BUTTON_R1) return;
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

    void showMicrophoneNotice() { hostActions.showMicrophoneNotice(); }

    String wifiText(long stallPeriodMs) { return hostActions.wifiText(stallPeriodMs); }

    com.limelight.binding.usb.UsbForwarding usbForwarding() { return hostActions.usbForwarding(); }

    void startUsbAutoForwarding() { hostActions.startUsbAutoForwarding(); }

    private void transferClipboard(boolean send) { hostActions.transferClipboard(send); }

    private void transferClipboardImage(boolean send) { hostActions.transferClipboardImage(send); }

    private void confirmQuitApp() {
        confirmHostExit(R.string.stream_quit_game, R.string.stream_quit_game_confirm, this::quitStreamApp);
    }

    private void confirmCloseGame() {
        confirmHostExit(R.string.stream_close_game, R.string.stream_close_game_confirm, this::quitStreamApp);
    }

    private void confirmEndHostSession() {
        confirmHostExit(R.string.stream_end_session, R.string.stream_end_session_confirm, this::endHostSession);
    }

    // A controller button in the stream menu that runs Disconnect asks first, as the ways out that end the game do:
    // a stray press must not end the stream.
    private void confirmDisconnect() {
        showStreamDialog(new NavigationDialogBuilder(game).setTitle(R.string.stream_disconnect)
                .setMessage(R.string.stream_disconnect_confirm).setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(R.string.stream_disconnect, (prompt, which) -> game.streamMenuAction = this::leaveStream).create());
    }

    private void confirmHostExit(int title, int message, Runnable exit) {
        showStreamDialog(hostExitDialog(game, title, message, () -> game.streamMenuAction = exit));
    }

    /**
     * The confirm for an action that ends the session or the game on the PC: the red of Home's confirms, not the moon (an
     * error-coloured label on the moon was unreadable).
     */
    static AlertDialog hostExitDialog(Context context, int title, int message, Runnable onConfirm) {
        AlertDialog dialog = new NavigationDialogBuilder(context).startOnCancel().setTitle(title)
                .setMessage(message).setNegativeButton(android.R.string.cancel, null)
                .setPositiveButton(title, (prompt, which) -> onConfirm.run()).create();
        com.limelight.utils.UiHelper.markDestructive(dialog);
        return dialog;
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
        final Object actionToken = GameAdaptation.beginHostAction(game);
        game.hostExitRequested = true;
        new Thread(() -> {
            try {
                exit.run();
                game.runOnUiThread(game::finish);
            } catch (IOException | XmlPullParserException | RuntimeException e) {
                game.runOnUiThread(this::confirmHostExitFailedLater);
                game.adaptation.showHostActionError(e, generation);
            } finally {
                GameAdaptation.endHostAction(game, actionToken);
            }
        }, threadName).start();
    }

    /**
     * Leaves the stream as the player asked. A game set to quit on leave is quit first, while the
     * stream still holds the host connection; a failure is reported because the game stays running.
     */
    void leaveStream() {
        if (com.limelight.console.OpenPlay.isAutoStart(game.getIntent())) {
            com.limelight.console.OpenPlay.streamEnded(game, 0, true);
        }
        boolean chosen = AppQuitOnLeave.isEnabled(game, game.getIntent().getStringExtra(Game.EXTRA_PC_UUID),
                game.app.getAppUuid(), game.app.getAppId());
        // A plain disconnect never waits for a host request; its callbacks check the activity.
        if (!com.limelight.nvstream.jni.NightjarPolicyJni.exitQuitsOnLeave(chosen, game.app.getRole().ordinal(),
                game.connected, DualScreen.isSecond(game.getIntent()))) {
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
        final Object actionToken = GameAdaptation.beginHostAction(game);
        game.hostExitRequested = true;
        Toast.makeText(context, game.getString(R.string.stream_quit_on_leave_progress, name), Toast.LENGTH_SHORT).show();
        new Thread(() -> {
            Exception failure = null;
            try {
                game.conn.closeGame();
            } catch (IOException | XmlPullParserException | RuntimeException e) {
                failure = e;
            }
            Exception error = failure;
            game.runOnUiThread(() -> {
                GameAdaptation.endHostActionOnUi(game, actionToken);
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
        final Object actionToken = GameAdaptation.beginHostAction(game);
        game.hostExitRequested = true;
        new Thread(() -> {
            try {
                game.conn.disconnectRemoteSession();
                game.runOnUiThread(game::finish);
            } catch (IOException | XmlPullParserException | RuntimeException e) {
                game.runOnUiThread(this::confirmHostExitFailedLater);
                game.adaptation.showHostActionError(e, generation);
            } finally {
                GameAdaptation.endHostAction(game, actionToken);
            }
        }, "End remote session").start();
    }

    private void confirmHostExitFailedLater() {
        game.bitrateHandler.removeCallbacks(hostExitFailed);
        game.bitrateHandler.postDelayed(hostExitFailed, HOST_EXIT_CONFIRM_MS);
    }

    private void showServerCommands(ComputerDetails details) {
        String[] names = details.serverCommands.subList(0, Math.min(256, details.serverCommands.size())).toArray(new String[0]);
        showStreamDialog(new NavigationDialogBuilder(game).setTitle(R.string.stream_server_commands)
                .setItems(names, (dialog, index) -> {
                    dialog.dismiss();
                    showStreamDialog(new NavigationDialogBuilder(game).startOnCancel().setTitle(names[index])
                            .setMessage(R.string.stream_command_confirm)
                            .setNegativeButton(android.R.string.cancel, null)
                            .setPositiveButton(R.string.stream_command_run, (confirmation, which) -> {
                                if (!game.foreground || !game.connected || game.hostActionInProgress) {
                                    return;
                                }
                                final Object actionToken = GameAdaptation.beginHostAction(game);
                                final int generation = game.foregroundGeneration;
                                new Thread(() -> {
                                    try {
                                        boolean sent = game.conn.sendServerCommand(index, names[index]);
                                        game.runOnUiThread(() -> {
                                            if (game.foreground && generation == game.foregroundGeneration && game.connected && !game.isFinishing()) {
                                                game.messages.alert(sent ? R.string.stream_command_sent : R.string.stream_command_failed, Toast.LENGTH_LONG);
                                            }
                                        });
                                    } catch (IOException | XmlPullParserException | RuntimeException e) {
                                        game.adaptation.showHostActionError(e, generation);
                                    } finally {
                                        GameAdaptation.endHostAction(game, actionToken);
                                    }
                                }, "Server command").start();
                            }).create());
                }).setNegativeButton(android.R.string.cancel, null).create());
    }
}
