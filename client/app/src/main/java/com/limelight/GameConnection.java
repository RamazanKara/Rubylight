package com.limelight;

import com.limelight.binding.audio.AndroidAudioRenderer;
import com.limelight.binding.audio.SilentAudioRenderer;
import com.limelight.binding.video.DecoderSetupFailure;
import com.limelight.binding.video.PackedFallback;
import com.limelight.binding.video.DiscardingDecoderRenderer;
import com.limelight.binding.video.DisplayCapsReporter;
import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.LaunchConfirmation;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.utils.NetworkGrace;
import com.limelight.utils.ServerHelper;
import com.limelight.utils.ShortcutHelper;
import com.limelight.utils.SpinnerDialog;
import com.limelight.utils.UiHelper;
import androidx.appcompat.app.AlertDialog;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.SystemClock;
import android.view.Display;
import android.view.Surface;
import android.view.TextureView;
import android.view.View;
import android.view.WindowManager;
import android.widget.TextView;
import android.widget.Toast;

/** The stream's connection: starting it on the video surface, the connection's stages and status, its messages and launch confirmations, losing and regaining the network, and stopping it. */
final class GameConnection {
    private final Game game;

    GameConnection(Game game) {
        this.game = game;
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
