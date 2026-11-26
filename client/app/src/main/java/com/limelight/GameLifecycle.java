package com.limelight;

import android.annotation.TargetApi;
import android.content.Context;
import android.content.Intent;
import android.content.res.Configuration;
import android.hardware.display.DisplayManager;
import android.hardware.input.InputManager;
import android.os.Build;
import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.View;
import android.view.WindowManager;
import android.widget.Toast;

import com.limelight.nvstream.http.NvApp;
import com.limelight.ui.StreamOrientation;
import com.limelight.utils.Dialog;
import com.limelight.utils.SpinnerDialog;
import com.limelight.utils.UiHelper;

/**
 * Activity lifecycle, configuration/PiP transitions, window focus, multi-window handling,
 * system-key reapplication, back navigation, and stop/destroy teardown extracted from {@link Game}.
 */
final class GameLifecycle {
    private final Game game;
    private boolean restoreInputOnResume;
    private boolean wasInPictureInPicture;

    GameLifecycle(Game game) {
        this.game = game;
    }

    void onConfigurationChanged(Configuration newConfig) {
        game.videoOutput.configureStreamWindow();
        if (game.controllerHandler == null || game.isFinishing()) {
            return;
        }

        // Set requested orientation for possible new screen size
        game.videoOutput.setPreferredOrientationForCurrentDisplay();
        // Attaching a hardware keyboard or switching the IME's language changes the layout.
        game.keyboardInput.reportKeyboardLayout();

        // A fold, rotation or window change may change the native size
        boolean turned = newConfig.orientation != game.lastWindowOrientation;
        game.lastWindowOrientation = newConfig.orientation;
        if (turned) game.videoOutput.noteRotationWithoutReconfigure();
        game.videoOutput.scheduleReconfigureCheck(StreamOrientation.settleMs(game.streamOrientation, game.hostShapesStream, turned,
                StreamReconfigureRequester.SETTLE_MS));

        // The on-screen controller follows its frame's size by itself, but portrait and landscape have their own layouts.
        if (game.modernController != null && game.prefConfig.onscreenController) {
            game.modernController.onOrientationChanged();
        }

        game.videoOutput.applyVideoScaling();

        // Hide on-screen overlays in PiP mode
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            boolean inPictureInPicture = game.isInPictureInPictureMode();
            boolean leftPictureInPicture = wasInPictureInPicture && !inPictureInPicture;
            wasInPictureInPicture = inPictureInPicture;
            if (inPictureInPicture) {
                game.isHidingOverlays = true;
                if (game.gameTouchControls != null) game.gameTouchControls.hideForPip(true);

                if (game.modernController != null) {
                    game.modernController.finishEditing(false);
                    game.modernController.hide();
                }
                game.touchInput.updateMicButton();
                game.touchInput.updateMenuButton();
                if (game.keyboardOverlay != null) {
                    game.keyboardInput.hideKeyboardOverlay();
                }
                game.keyboardInput.hidePcKeyboard();

                game.performanceOverlayScroll.setVisibility(View.GONE);
                game.notificationOverlayView.setVisibility(View.GONE);
                game.overlays.dismissStreamMenuHint(false);

                // Disable sensors while in PiP mode
                game.controllerHandler.disableSensors();

                if (game.trackpadPointer != null) {
                    game.trackpadPointer.stopMomentum();
                    game.trackpadPointer.setCursorShown(false);
                }
                if (game.desktopGestures != null) {
                    game.desktopGestures.reset();
                }

                // Update GameManager state to indicate we're in PiP (still gaming, but interruptible)
                UiHelper.notifyStreamEnteringPiP(game);
            }
            // Rotation, folding, a keyboard or dark mode also change the configuration; only a real
            // way out of PiP restores what it hid.
            else if (leftPictureInPicture) {
                game.isHidingOverlays = false;
                boolean canSendInput = game.conn != null && game.conn.canSendInput();
                if (game.trackpadPointer != null) game.trackpadPointer.setCursorShown(true);
                // A session without input permission keeps its controls hidden.
                if (game.gameTouchControls != null && canSendInput) game.gameTouchControls.hideForPip(false);

                // Restore overlays to previous state when leaving PiP

                if (game.onscreenControlsShown() && canSendInput) {
                    game.showOnscreenController();
                }
                game.touchInput.updateMicButton();
                game.touchInput.updateMenuButton();

                if (game.prefConfig.enablePerfOverlay && game.app.getRole() != NvApp.Role.INPUT_ONLY) {
                    game.performanceOverlayScroll.setVisibility(View.VISIBLE);
                }

                game.notificationOverlayView.setVisibility(game.requestedNotificationOverlayVisibility);

                // Enable sensors again after exiting PiP
                game.controllerHandler.enableSensors();

                // Update GameManager state to indicate we're out of PiP (gaming, non-interruptible)
                UiHelper.notifyStreamExitingPiP(game);
            }
        }
        game.videoOutput.updateCompositorHeartbeat();
        game.overlays.updateHardwareMetricsPolling();
    }

    /**
     * "System keys only in full screen": the grab follows the window. Keys held across the change go up first, so none
     * stays down on the PC once Android takes the keys back.
     */
    void reapplySystemKeys() {
        if (game.prefConfig == null || game.keyboardInput == null) return;
        if (!game.prefConfig.captureSystemKeys || !game.prefConfig.systemKeysFullscreenOnly) return;
        if (game.conn != null && game.touchInput != null) game.keyboardInput.releaseKeyboardInput();
        game.setMetaKeyCaptureState(game.grabbedInput && game.prefConfig.sendsSystemKeys(game.inWindow()));
    }

    void onUserLeaveHint() {
        // PiP is only supported on Oreo and later, and we don't need to manually enter PiP on
        // Android S and later. On Android R, we will use onPictureInPictureRequested() instead.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            if (game.autoEnterPip) {
                try {
                    // This has thrown all sorts of weird exceptions on Samsung devices
                    // running Oreo. Just eat them and close gracefully on leave, rather
                    // than crashing.
                    game.enterPictureInPictureMode(game.videoOutput.getPictureInPictureParams(false));
                } catch (Exception e) {
                    e.printStackTrace();
                }
            }
        }
    }

    @TargetApi(Build.VERSION_CODES.R)
    boolean onPictureInPictureRequested() {
        // Enter PiP when requested unless we're on Android 12 which supports auto-enter.
        if (game.autoEnterPip && Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
            try {
                game.enterPictureInPictureMode(game.videoOutput.getPictureInPictureParams(false));
            } catch (IllegalStateException | IllegalArgumentException e) {
                LimeLog.warning("Picture-in-picture unavailable");
            }
        }
        return true;
    }

    boolean isDesktopPointerTouch(MotionEvent event) {
        return event.isFromSource(InputDevice.SOURCE_CLASS_POINTER) &&
                event.getActionMasked() != MotionEvent.ACTION_CANCEL &&
                (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
                        (event.getFlags() & MotionEvent.FLAG_CANCELED) == 0);
    }

    void onWindowFocusChanged(boolean hasFocus) {
        game.desktopWindowFocused = hasFocus;

        if (!hasFocus) {
            game.touchInput.cancelTouchInput();
            game.keyboardInput.releaseKeyboardInput();
            game.touchInput.releaseMouseButtons();
            if (game.modernController != null) {
                game.modernController.releaseInput();
            }
        }
        if (game.controllerHandler != null) {
            game.controllerHandler.setInputEnabled(hasFocus && game.grabbedInput);
        }

        // We can't guarantee the state of modifiers keys which may have
        // lifted while focus was not on us. Clear the modifier state.
        game.modifierFlags = 0;

        // With Android native pointer capture, capture is lost when focus is lost,
        // so it must be requested again when focus is regained.
        if (game.inputCaptureProvider != null) {
            game.inputCaptureProvider.onWindowFocusChanged(hasFocus);
        }
        // A copy made in another app goes once the stream is back in front.
        if (game.clipboardSync != null) {
            game.clipboardSync.focusChanged(hasFocus);
        }
        if (hasFocus) {
            // Let Android finish restoring IME focus before running a menu action.
            game.getWindow().getDecorView().post(game.runWindowFocusActions);
        }
    }

    void onMultiWindowModeChanged(boolean isInMultiWindowMode) {
        reapplySystemKeys();
        game.videoOutput.configureStreamWindow();
        if (game.decoderRenderer == null) {
            return;
        }

        // In multi-window, we don't want to use the full-screen layout
        // flag. It will cause us to collide with the system UI.
        // This function will also be called for PiP so we can cover
        // that case here too.
        if (isInMultiWindowMode) {
            game.getWindow().clearFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN);
        } else {
            game.getWindow().addFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN);
        }

        game.videoOutput.updatePipAutoEnter();
        game.videoOutput.scheduleReconfigureCheck();

        // Correct the system UI visibility flags
        game.videoOutput.hideSystemUi(50);
    }

    void onDestroy() {
        DualScreen.end(game);
        game.safeArea.detach();
        if (game.nativeMenuThread != null) game.nativeMenuThread.shutdownNow();
        if (game.streamHealthThread != null) game.streamHealthThread.shutdownNow();
        game.stagedExitController.onDestroy();
        // A message still waiting for its turn would show over the screen after the stream.
        game.messages.dropWaiting();
        game.streamConnection.stopConnection();
        // Nothing of TV display compatibility outlives the screen, the stream ended or not.
        if (game.videoOutput.compositorHeartbeat != null) {
            game.videoOutput.compositorHeartbeat.stop();
            game.videoOutput.compositorHeartbeat = null;
        }
        if (game.externalPresentation != null) {
            game.externalPresentation.setOnDismissListener(null);
            game.externalPresentation.dismiss();
            game.externalPresentation = null;
        }
        ((DisplayManager) game.getSystemService(Context.DISPLAY_SERVICE))
                .unregisterDisplayListener(game.inputDisplayListener);
        if (game.streamMenu != null) { game.streamMenu.dismiss(); }
        if (game.spinner != null) { game.spinner.dismiss(); game.spinner = null; }
        SpinnerDialog.closeDialogs(game);
        Dialog.closeDialogs(game);
        game.getWindow().getDecorView().removeCallbacks(game.hideSystemUi);
        game.getWindow().getDecorView().removeCallbacks(game.toggleGrab);
        if (game.pipDisconnectIntent != null) {
            game.unregisterReceiver(game.pipReceiver);
            game.pipDisconnectIntent.cancel();
        }
        game.bitrateHandler.removeCallbacksAndMessages(null);
        game.keyboardPanHandler.removeCallbacks(game.keyboardPan);
        if (game.legacyKeyboard != null) game.legacyKeyboard.stop();

        game.streamConnection.releaseUsbDriverBinder();
        if (game.controllerHandler != null) {
            game.controllerHandler.destroy();
        }
        if (game.keyboardTranslator != null) {
            InputManager inputManager = (InputManager) game.getSystemService(Context.INPUT_SERVICE);
            inputManager.unregisterInputDeviceListener(game.keyboardTranslator);
            inputManager.unregisterInputDeviceListener(game.pcKeyboardDeviceListener);
        }

        if (game.usbDriverServiceBound) {
            game.unbindService(game.usbDriverServiceConnection);
            game.usbDriverServiceBound = false;
        }

        // Destroy the capture provider
        if (game.inputCaptureProvider != null) {
            game.inputCaptureProvider.destroy();
        }
        LimeLog.info("Stream activity destroyed");
    }

    void onNewIntent(Intent intent) {
        // singleTask can deliver a new shortcut launch to a stream still visible in PiP.
        game.touchInput.setInputGrabState(false);
        game.streamConnection.stopConnection();
        game.streamRecovery.handOff(new Intent(intent));
    }

    void onResume() {
        game.foreground = true;
        game.videoOutput.updateCompositorHeartbeat();
        game.videoOutput.armHdrRecomposition();
        if (game.streamRecovery.resumeStoppedStreamIfNeeded()) {
            game.recoveryCancelledOnPause = false;
            restoreInputOnResume = false;
            return;
        }
        if (game.recoveryCancelledOnPause && !game.connected && !game.isFinishing()) {
            game.displayedFailureDialog = true;
            game.streamConnection.stopConnection();
            if (game.spinner != null) { game.spinner.dismiss(); game.spinner = null; }
            game.streamRecovery.showReconnectDialog(R.string.conn_terminated_title,
                    game.getString(R.string.stream_recovery_paused));
        }
        game.recoveryCancelledOnPause = false;
        if (restoreInputOnResume && game.connected) {
            game.touchInput.setInputGrabState(true);
        }
        restoreInputOnResume = false;
        game.overlays.updateHardwareMetricsPolling();
    }

    void onPause() {
        game.touchInput.releaseDesktopInput();
        if (game.trackpadPointer != null) game.trackpadPointer.stopMomentum();
        if (game.gameTouchControls != null) game.gameTouchControls.releaseInput();
        // Shown again on the next stream unless it was read.
        game.overlays.dismissStreamMenuHint(false);
        game.foreground = false;
        game.videoOutput.updateCompositorHeartbeat();
        game.recoveryCancelledOnPause = game.recoveryHandoffInProgress || game.pendingRecovery != null
                || (game.streamRecovery.isRecoveryAttempt() && game.connecting);
        game.streamRecovery.cancelAutomaticRecovery();
        game.overlays.stopHardwareMetricsPolling();
        game.foregroundGeneration++;
        game.streamMenuAction = null;
        game.windowFocusActions.clear();
        game.getWindow().getDecorView().removeCallbacks(game.runWindowFocusActions);
        if (game.pendingConfirmation != null) {
            game.pendingConfirmation.cancel();
            game.launchConfirmationFinished();
        }
        if (game.streamMenu != null && (!game.menuActions.isTerminalDialog() || game.isFinishing())) {
            restoreInputOnResume = game.restoreInputAfterMenu;
            game.streamMenu.dismiss();
        }
        if (game.isFinishing()) {
            // Stop any further input device notifications before we lose focus (and pointer capture)
            if (game.controllerHandler != null) {
                game.controllerHandler.stop();
            }

            // Ungrab input to prevent further input device notifications
            game.touchInput.setInputGrabState(false);
        }
    }

    void onStopAfterLinkGate() {
        if (game.adaptiveLayout != null) game.adaptiveLayout.stop();
        game.streamRecovery.rememberStreamForResume(true);

        if (game.streamMenu != null) {
            game.streamMenu.dismiss();
        }

        if (game.spinner != null) { game.spinner.dismiss(); game.spinner = null; }
        SpinnerDialog.closeDialogs(game);
        Dialog.closeDialogs(game);

        if (game.modernController != null) {
            game.modernController.finishEditing(false);
            game.modernController.hide();
        }
        if (game.micButton != null) game.micButton.setSuppressed(true);
        if (game.menuButton != null) game.menuButton.setShown(false);

        if (game.conn != null) {
            int videoFormat = game.decoderRenderer.getActiveVideoFormat();

            game.displayedFailureDialog = true;
            game.streamConnection.stopConnection();

            if (game.prefConfig.enableLatencyToast) {
                String message = GameStatusText.latencyToast(game, videoFormat,
                        game.decoderRenderer.getAverageEndToEndLatency(),
                        game.decoderRenderer.getAverageDecoderLatency(),
                        game.streamSdr10, game.decoderRenderer.getHostHdrReport());
                if (message != null) {
                    game.messages.alert(message, Toast.LENGTH_LONG);
                }
            }

            // Clear the tombstone count if we terminated normally
            synchronized (game.tombstonePrefs) {
                if (!game.reportedCrash && game.tombstonePrefs.getInt("CrashCount", 0) != 0) {
                    game.tombstonePrefs.edit()
                            .putInt("CrashCount", 0)
                            .putInt("LastNotifiedCrashCount", 0)
                            .remove("CrashHostUuid")
                            .apply();
                }
            }
        }

        if (game.streamRecovery.finishesWhenStopped()) game.finish();
    }

    /**
     * Back: leaves the layout editor, the PC keyboard or the system keyboard, else opens the menu, focused when a
     * controller or remote sent it. Returns true if consumed.
     */
    boolean onBack(boolean byController) {
        if (game.modernController != null && game.modernController.isEditing()) {
            // Back leaves the layout editor without keeping its changes, as Cancel does.
            game.modernController.finishEditing(false);
        } else if (game.connected && game.keyboardInput.isPcKeyboardVisible()) {
            game.keyboardInput.hidePcKeyboard();
        } else if (game.connected && game.keyboardInput.isSystemKeyboardVisible()) {
            // A touch Back goes to the keyboard itself; a remote's Back reaches the stream first and comes here.
            game.keyboardInput.closeSystemKeyboard();
        } else if (game.connected) {
            if (byController) game.showStreamMenuFromController();
            else game.showStreamMenu();
        } else {
            return false;
        }
        return true;
    }
}
