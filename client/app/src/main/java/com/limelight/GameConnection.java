/**
 * Stream connection lifecycle coordinator: surface bind/teardown, startup stage reporting,
 * network loss/return recovery, and delegation to {@link GameTerminationFlow} and
 * {@link NightjarSessionJni}.
 */
package com.limelight;

import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.graphics.Bitmap;
import android.net.ConnectivityManager;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.view.Display;
import android.view.Surface;
import android.view.View;
import android.view.WindowManager;
import android.widget.TextView;
import android.widget.Toast;

import androidx.annotation.VisibleForTesting;
import androidx.appcompat.app.AlertDialog;

import com.limelight.binding.audio.AndroidAudioRenderer;
import com.limelight.binding.audio.SilentAudioRenderer;
import com.limelight.binding.video.DiscardingDecoderRenderer;
import com.limelight.binding.video.DisplayCapsReporter;
import com.limelight.binding.video.FramePacingPolicy;
import com.limelight.binding.video.RefreshSwitchMonitor;
import com.limelight.console.OpenPlay;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nvstream.av.audio.AudioRenderer;
import com.limelight.nvstream.av.video.VideoDecoderRenderer;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.LaunchConfirmation;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarPolicyJni;
import com.limelight.nvstream.jni.NightjarSessionJni;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.ConnectingScreen;
import com.limelight.ui.StreamMenuHint;
import com.limelight.utils.ClipboardSync;
import com.limelight.utils.ShortcutHelper;
import com.limelight.utils.UiHelper;
import com.limelight.utils.WifiDiagnostics;

import java.net.InetAddress;

final class GameConnection {
    private final Game game;
    AlertDialog launchConfirmationDialog;
    private boolean streamNetworkLost;
    private volatile Boolean androidTv;
    volatile Bitmap connectingCover;
    volatile Bitmap menuCover;
    HostLossWatch hostLossWatch;
    private Runnable openPlaySettled;
    boolean streamFailed;
    volatile String movedAwayTo;
    long moveWatchHandle;
    Runnable moveWatchTask;
    android.app.Dialog moveWatchDialog;

    static final long STOP_REPORT_MS = 10_000;
    static final long USB_DETACH_WAIT_MS = 500;
    @VisibleForTesting static final int MOVE_WATCH_READS = 50;

    GameConnection(Game game) { this.game = game; }

    void destroyMoveWatch() {
        if (moveWatchTask != null) {
            game.bitrateHandler.removeCallbacks(moveWatchTask);
            moveWatchTask = null;
        }
        moveWatchDialog = null;
        if (moveWatchHandle != 0) {
            long handle = moveWatchHandle;
            moveWatchHandle = 0;
            NightjarSessionJni.moveWatchDestroy(handle);
        }
    }

    void onCreate(Bundle savedInstanceState) {
        if (!GameStreamUiSetup.onCreate(game, this, savedInstanceState)) {
            streamFailed = true;
            game.finish();
        }
    }

    private void requestHostTiming() {
        if (game.conn == null || !game.conn.getHostDetails().caps.has(HostCapabilities.Feature.HOST_TIMING)
                || !MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_HOST_TIMING)) {
            return;
        }
        MoonBridge.sendHostTimingRequest(true);
    }

    private void requestLinkNotice() {
        game.linkNotice = null;
        game.linkLoweredTold = false;
        if (game.conn == null || !MoonBridge.hostSupportsControlMessage(MoonBridge.CONTROL_MESSAGE_LINK_NOTICE)) {
            return;
        }
        MoonBridge.sendLinkNoticeRequest(true);
    }

    private void startClipboardSync() {
        if (game.clipboardSync != null) { game.clipboardSync.stop(); game.clipboardSync = null; }
        if (game.conn == null || !game.conn.askedForClipboardSync()) return;
        game.clipboardSync = ClipboardSync.forStream(game, text -> game.messages.alert(text, Toast.LENGTH_LONG));
        game.clipboardSync.connectionStarted(game.hasWindowFocus());
    }

    int streamFrameRate(float displayRefreshRate) {
        int fps = game.prefConfig.fps;
        if (game.prefConfig.framePacing != PreferenceConfiguration.FRAME_PACING_CAP_FPS) return fps;
        float limitHz = FramePacingPolicy.limitRefreshHz(
                game.desiredRefreshRate, displayRefreshRate, fps, RefreshSwitchMonitor.conditions(game));
        int limited = FramePacingPolicy.limitedFrameRate(fps, limitHz);
        if (limited == FramePacingPolicy.LIMIT_ABOVE_SCREEN) {
            game.prefConfig.framePacing = PreferenceConfiguration.FRAME_PACING_BALANCED;
            LimeLog.info("Using drop mode for FPS > Hz");
            return fps;
        }
        if (limited == FramePacingPolicy.LIMIT_BOGUS_SCREEN) {
            game.prefConfig.framePacing = PreferenceConfiguration.FRAME_PACING_BALANCED;
            LimeLog.info("Bogus refresh rate: " + Math.round(limitHz));
            return fps;
        }
        if (limited != fps) LimeLog.info("Adjusting FPS target for screen to " + limited);
        return limited;
    }

    boolean isAndroidTv() {
        Boolean tv = androidTv;
        if (tv == null) {
            tv = game.getPackageManager().hasSystemFeature(PackageManager.FEATURE_TELEVISION)
                    || game.getPackageManager().hasSystemFeature(PackageManager.FEATURE_LEANBACK);
            androidTv = tv;
        }
        return tv;
    }

    boolean streamEnding() { return game.streamTerminated || game.exiting || game.connectionStopThread != null; }

    void noteVideoDisplay(Display display, boolean removed) {
        game.videoDisplayOff = removed || !display.isValid() || display.getState() != Display.STATE_ON;
        game.stagedExit.onDisplayChanged(SystemClock.uptimeMillis());
    }

    ConnectingScreen showConnecting(String detail) {
        if (game.spinner != null) {
            game.spinner.dismiss();
            game.spinner = null;
        }
        Intent intent = game.getIntent();
        ConnectingScreen screen = ConnectingScreen.show(game, intent.getStringExtra(Game.EXTRA_APP_NAME),
                intent.getStringExtra(Game.EXTRA_PC_NAME), game.getString(R.string.conn_establishing_title),
                detail, game::finish);
        if (screen == null) return null;
        GameStreamUiSetup.readCoverInto(game, this, screen);
        return screen;
    }

    void readCoverInto(ConnectingScreen screen) { GameStreamUiSetup.readCoverInto(game, this, screen); }
    Bitmap connectingCover() { Bitmap full = connectingCover; return full != null ? full : menuCover; }
    void dropConnectingCover() { connectingCover = null; }

    void stageStarting(final String stage) {
        game.runOnUiThread(() -> {
            if (game.spinner != null) game.spinner.setStage(game.getResources().getString(stageText(stage)));
        });
    }

    static int stageText(String stage) {
        String key = NightjarSessionJni.stageTextKey(stage);
        if (key == null) return R.string.conn_starting;
        switch (key) {
            case "finding_pc": return R.string.conn_stage_finding;
            case "setting_up_stream": return R.string.conn_stage_setup;
            case "starting_stream": return R.string.conn_stage_stream;
            case "starting_video": return R.string.conn_stage_video;
            case "starting_audio": return R.string.conn_stage_audio;
            case "connecting_input": return R.string.conn_stage_input;
            default: return R.string.conn_starting;
        }
    }

    private void startStreamGuests() {
        if (!game.connected || game.isFinishing() || game.streamGuests != null
                || !game.conn.offersSessionGuests() || game.app.getRole() != NvApp.Role.STREAM) {
            return;
        }
        NativeApiClient client = game.conn.nativeApiClient();
        if (client == null) return;
        game.streamGuests = new StreamGuests(game, client);
        game.streamGuests.start();
    }

    private void startHostLossWatch() {
        if (hostLossWatch != null) hostLossWatch.stop();
        hostLossWatch = null;
        if (game.conn == null) return;
        if (!MoonBridge.hostLossAvailable()) {
            LimeLog.warning("A PC that shuts down is told only by the recovery running out: libnightjar_core.so did not load");
            return;
        }
        InetAddress host = HostLossWatch.parse(MoonBridge.getHostAddress());
        if (host == null) {
            LimeLog.warning("A PC that shuts down is told only by the recovery running out: the stream's address is unknown");
            return;
        }
        ConnectivityManager connectivity = (ConnectivityManager) game.getSystemService(Context.CONNECTIVITY_SERVICE);
        hostLossWatch = new HostLossWatch(connectivity, host, game.conn.hostHttpsPort());
    }

    void stopConnection() {
        destroyMoveWatch();
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
        if (game.lowLatencyWifiLock != null && game.lowLatencyWifiLock.isHeld()) game.lowLatencyWifiLock.release();
        if (game.highPerfWifiLock != null && game.highPerfWifiLock.isHeld()) game.highPerfWifiLock.release();
        game.streamMenuAction = null;
        game.windowFocusActions.clear();
        game.getWindow().getDecorView().removeCallbacks(game.runWindowFocusActions);
        game.bitrateHandler.removeCallbacks(game.updateBitrate);
        game.bitrateHandler.removeCallbacks(game.updateRateChangeBitrate);
        game.bitrateHandler.removeCallbacks(game.expireRateChanges);
        if (game.rateChanges != null) game.rateChanges.stopAutomatic();
        game.rateChanges = null;
        game.bitrateHandler.removeCallbacks(game.checkReconfigure);
        game.bitrateHandler.removeCallbacks(game.updateQuality);
        game.videoOutput.clearDecoderActions();
        if (game.adaptiveQuality != null) game.adaptiveQuality.close();
        game.adaptiveQuality = null;
        game.pendingQualityTarget = null;
        if (game.adaptiveBitrate != null) game.adaptiveBitrate.close();
        game.adaptiveBitrate = null;
        game.qualityAutoBitrate = false;
        game.bitrateHandler.removeCallbacks(game.adaptation.sampleStreamHealth);
        game.bitrateHandler.removeCallbacks(game.videoOutput.pollContentRateVote);
        game.videoOutput.stopRefreshSwitchPolling();
        if (game.wifiDiagnostics != null) game.wifiDiagnostics.stop();
        if (game.pyroWaveBitrate != null) game.pyroWaveBitrate.close();
        game.pyroWaveBitrate = null;
        game.displayCapsReporter = null;
        game.touchInput.stopHostCursorShapes();
        if (game.clipboardSync != null) { game.clipboardSync.stop(); game.clipboardSync = null; }
        if (game.streamGuests != null) { game.streamGuests.stop(); game.streamGuests = null; }
        com.limelight.binding.usb.UsbForwarding detaching = game.usbForwarding;
        if (detaching != null) { detaching.stop(); game.usbForwarding = null; }
        releaseUsbDriverBinder();
        if (game.connecting || game.connected) {
            game.connecting = game.connected = false;
            game.videoOutput.updatePipAutoEnter();
            game.videoOutput.updateCompositorHeartbeat();
            if (game.controllerHandler != null) game.controllerHandler.stop();
            if (game.conn.isMicrophoneMutedForReconnect()) game.getIntent().putExtra(Game.EXTRA_MICROPHONE_MUTED, true);
            UiHelper.notifyStreamEnded(game);
            com.limelight.nvstream.NvConnection stoppingConn = game.conn;
            Thread stopping = new Thread(() -> {
                if (detaching != null && !detaching.awaitDetaches(USB_DETACH_WAIT_MS)) {
                    LimeLog.warning("USB: detaches still running when the stream stopped");
                }
                stoppingConn.stop();
            }, "Stop stream");
            game.connectionStopThread = stopping;
            stopping.start();
            LimeLog.info("Stream stopping");
            new Handler(Looper.getMainLooper()).postDelayed(() -> {
                String stuck = stuckStopReport(stopping);
                if (stuck != null) LimeLog.severe(stuck);
            }, STOP_REPORT_MS);
        }
    }

    static String stuckStopReport(Thread stopping) {
        if (!stopping.isAlive()) return NightjarSessionJni.sessionStuckStopReport(false, "");
        StringBuilder lines = new StringBuilder();
        for (StackTraceElement frame : stopping.getStackTrace()) lines.append("\n  at ").append(frame);
        return NightjarSessionJni.sessionStuckStopReport(true, lines.toString());
    }

    void releaseUsbDriverBinder() {
        game.usbDriverReleased = true;
        if (game.usbDriverBinder != null) {
            game.usbDriverBinder.setListener(null);
            game.usbDriverBinder.setStateListener(null);
            game.usbDriverBinder.stop();
            game.usbDriverBinder = null;
        }
    }

    void streamMoved(String device) {
        LimeLog.info("The resume stopped: the stream moved to " + device);
        movedAwayTo = device;
    }

    void stageFailed(final String stage, final int portFlags, final int errorCode) {
        GameTerminationFlow.stageFailed(game, this, stage, portFlags, errorCode);
    }

    void connectionTerminated(final int reportedErrorCode) {
        GameTerminationFlow.connectionTerminated(game, this, reportedErrorCode);
    }

    void connectionStatusUpdate(final int connectionStatus) {
        game.runOnUiThread(() -> {
            if (!game.connected || game.isFinishing() || game.isDestroyed()) return;
            game.poorConnection = connectionStatus == MoonBridge.CONN_STATUS_POOR;
            if (game.prefConfig.disableWarnings || streamNetworkLost) return;
            if (connectionStatus == MoonBridge.CONN_STATUS_POOR) {
                game.notificationOverlayView.setText(game.getResources().getString(
                        game.prefConfig.bitrate > 5000 ? R.string.slow_connection_msg : R.string.poor_connection_msg));
                game.requestedNotificationOverlayVisibility = View.VISIBLE;
            } else if (connectionStatus == MoonBridge.CONN_STATUS_OKAY) {
                game.requestedNotificationOverlayVisibility = View.GONE;
            }
            if (!game.isHidingOverlays) game.notificationOverlayView.setVisibility(game.requestedNotificationOverlayVisibility);
        });
    }

    void streamNetworkLost() {
        game.runOnUiThread(() -> {
            if (!game.connected || game.displayedFailureDialog || game.isFinishing() || game.isDestroyed()) return;
            streamNetworkLost = true;
            boolean promises = game.streamRecovery.recoversAutomatically()
                    && game.prefConfig.networkGraceSeconds * 1000 <= NightjarPolicyJni.recoveryPeerTimeoutMs();
            game.notificationOverlayView.setText(game.getString(
                    promises ? R.string.stream_network_lost_reconnecting : R.string.stream_network_lost));
            game.requestedNotificationOverlayVisibility = View.VISIBLE;
            if (!game.isHidingOverlays) game.notificationOverlayView.setVisibility(View.VISIBLE);
        });
    }

    void streamNetworkReturned(final int change, final boolean metered) {
        game.runOnUiThread(() -> {
            if (streamNetworkLost) {
                streamNetworkLost = false;
                game.requestedNotificationOverlayVisibility = View.GONE;
                if (!game.isHidingOverlays) game.notificationOverlayView.setVisibility(View.GONE);
            }
            if (!game.connected || game.displayedFailureDialog || game.isFinishing() || game.isDestroyed()
                    || game.prefConfig == null || !game.streamRecovery.videoStarted()
                    || !NightjarPolicyJni.recoveryReconnectsAtOnce(
                            change == MoonBridge.NETWORK_CANNOT_CONTINUE, metered,
                            game.prefConfig.roamingMetered, game.prefConfig.autoReconnect,
                            game.prefConfig.networkGraceSeconds * 1000)) {
                return;
            }
            LimeLog.info("The network is back on another address the stream did not roam to: reconnecting now");
            if (!game.streamRecovery.tryAutomaticRecovery(false, true)) {
                LimeLog.warning("The network is back, but no automatic reconnection is possible now");
            }
        });
    }

    void playerLeft() {
        if (streamFailed || !OpenPlay.isAutoStart(game.getIntent())) return;
        if (game.streamRecovery.isRecoveryAttempt() && !game.connected) return;
        OpenPlay.streamEnded(game, 0, true);
    }

    void automaticStartStopped() { OpenPlay.streamEnded(game, 0, true); }

    void connectionStarted() {
        game.runOnUiThread(() -> {
            if (!game.connecting || game.isFinishing() || game.isDestroyed()) return;
            if (OpenPlay.isAutoStart(game.getIntent())) {
                if (openPlaySettled != null) game.bitrateHandler.removeCallbacks(openPlaySettled);
                openPlaySettled = OpenPlay.settleAfter(game, game.bitrateHandler);
            }
            dropConnectingCover();
            if (game.spinner != null) { game.spinner.dismiss(); game.spinner = null; }
            game.connected = true;
            game.videoOutput.updateCompositorHeartbeat();
            startHostLossWatch();
            game.conn.refreshNativeHostControl(() -> game.runOnUiThread(() -> {
                game.menuActions.startUsbAutoForwarding();
                startStreamGuests();
            }));
            game.getIntent().removeExtra(Game.EXTRA_RECOVERY_MEDIA_FAILURE);
            game.bitrateHandler.removeCallbacks(game.adaptation.sampleStreamHealth);
            game.bitrateHandler.postDelayed(game.adaptation.sampleStreamHealth, 1000);
            game.bitrateHandler.removeCallbacks(game.videoOutput.pollContentRateVote);
            if (game.prefConfig.contentRateFrameRate) game.bitrateHandler.postDelayed(game.videoOutput.pollContentRateVote, 1000);
            game.videoOutput.startRefreshSwitchPolling();
            if (game.wifiDiagnostics == null) game.wifiDiagnostics = new WifiDiagnostics(game);
            game.wifiDiagnostics.start();
            game.streamRecovery.onStreamConnected(SystemClock.elapsedRealtime());
            game.recoveryAppId = game.app.getControl() == NvApp.Control.NONE
                    ? game.app.getAppId() : game.conn.getHostDetails().runningGameId;
            game.recoveryAppUuid = game.app.getControl() == NvApp.Control.NONE
                    ? game.app.getAppUuid() : game.conn.getHostDetails().runningGameUuid;
            game.touchInput.updateGameTouchProfile();
            game.controllerHandler.resendControllerArrivals();
            game.overlays.updateHardwareMetricsPolling();
            LimeLog.info("Stream connected");
            game.connecting = false;
            DualScreen.start(game);
            if (!game.streamRecovery.isRecoveryAttempt()) {
                com.limelight.update.UpdateFlow.notifyForStream(game, game.conn.getHostDetails(),
                        () -> game.foreground && game.connected, text -> game.messages.alert(text, Toast.LENGTH_LONG));
                // The PC's setup fallback, once per launch: a recovery's resume does not repeat it.
                String warning = com.limelight.nvstream.http.LaunchWarning.text(
                        game.conn.getLaunchWarningCode(), game.conn.getLaunchWarningMessage(),
                        game.getString(R.string.launch_warning_virtual_display_unavailable),
                        game.getString(R.string.launch_warning_virtual_display_disabled_by_client));
                if (warning != null) {
                    game.messages.alert(warning, Toast.LENGTH_LONG);
                }
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
                if (game.modernController != null) game.modernController.hide();
                if (game.micButton != null) game.micButton.setSuppressed(true);
                game.controllerHandler.stop();
                game.touchInput.setInputGrabState(false);
            }
            if (game.app.isSmallStreamInputOnly() && game.externalPresentation == null) {
                ((TextView) game.findViewById(R.id.inputOnlyStatus)).setText(game.getString(
                        R.string.input_only_small_stream_status, NightjarPolicyJni.inputOnlyEstimatedKbps() / 1000f));
            }
            game.findViewById(R.id.inputOnlyStatus).setVisibility(
                    game.app.getRole() == NvApp.Role.INPUT_ONLY || game.externalPresentation != null ? View.VISIBLE : View.GONE);
            game.currentBitrate = game.app.isSmallStreamInputOnly() ? game.prefConfig.bitrate : game.conn.getStartBitrateKbps();
            game.overlays.notePyroWaveFallback();
            game.adaptation.startStreamBitrate();
            game.videoOutput.whenDecoderReady("start", game.adaptation::startAdaptiveQuality);
            game.videoOutput.updatePipAutoEnter();
            if (!game.conn.getHostDetails().hasPermission(ComputerDetails.PERMISSION_INPUT)) {
                game.messages.alert(R.string.stream_input_denied, Toast.LENGTH_LONG);
            }
            game.bitrateHandler.postDelayed(() -> {
                if (game.foreground && game.connected && game.streamMenu == null && !game.isFinishing()) {
                    game.touchInput.setInputGrabState(true);
                }
            }, 500);
            game.getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
            UiHelper.notifyStreamConnected(game);
            game.videoOutput.hideSystemUi(1000);
            if (!StreamMenuHint.seen(game)) {
                game.bitrateHandler.removeCallbacks(game.showStreamMenuHint);
                game.bitrateHandler.postDelayed(game.showStreamMenuHint, 1500);
            }
        });
        ComputerDetails computer = new ComputerDetails();
        computer.name = game.pcName;
        computer.uuid = game.getIntent().getStringExtra(Game.EXTRA_PC_UUID);
        ShortcutHelper shortcutHelper = new ShortcutHelper(game);
        shortcutHelper.reportComputerShortcutUsed(computer);
        if (game.appName != null) {
            shortcutHelper.reportGameLaunched(computer, game.app, game.getIntent().getStringExtra(Game.EXTRA_APP_ART_KEY));
        }
    }

    void displayMessage(final String message) {
        game.runOnUiThread(() -> {
            game.connectionErrorMessage = message;
            if (game.connected) game.messages.alert(message, Toast.LENGTH_LONG);
        });
    }

    void launchConfirmationRequired(LaunchConfirmation confirmation) {
        GameMoveFlow.launchConfirmationRequired(game, this, confirmation);
    }

    void launchConfirmationFinished() {
        game.runOnUiThread(() -> {
            game.pendingConfirmation = null;
            if (launchConfirmationDialog != null) { launchConfirmationDialog.dismiss(); launchConfirmationDialog = null; }
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

    void displayTransientMessage(final String message) {
        game.runOnUiThread(() -> {
            if (game.prefConfig.disableWarnings) game.messages.note(message);
            else game.messages.countedNotice(message);
        });
    }

    void startConnectionOnSurface() {
        if (!game.surfaceCreated || game.isFinishing() || game.isDestroyed()) return;
        if (!game.attemptedConnection) {
            if (game.streamRecovery.isRecoveryAttempt()) {
                long deadlineFrom = game.streamRecovery.isTripResume()
                        ? game.getIntent().getLongExtra(Game.EXTRA_TRIP_RESUME_STARTED, 0) : game.recoveryStartedAt;
                long remaining = NightjarPolicyJni.recoveryWindowMs() - (SystemClock.elapsedRealtime() - deadlineFrom);
                if (remaining <= 0) {
                    if (game.spinner != null) { game.spinner.dismiss(); game.spinner = null; }
                    game.streamRecovery.showReconnectDialog(
                            R.string.conn_terminated_title, game.getString(R.string.stream_recovery_exhausted));
                    return;
                }
                game.bitrateHandler.postDelayed(game.recoveryTimeout, remaining);
            }
            game.attemptedConnection = true;
            game.connecting = true;
            LimeLog.info("Stream starting");
            UiHelper.notifyStreamConnecting(game);
            game.decoderRenderer.setRenderTarget(game.videoSurface, game.renderView.getWidth(), game.renderView.getHeight());
            Renderers renderers = renderers(game.app.isSmallStreamInputOnly(), game.prefConfig.muteAudio,
                    game.decoderRenderer, game, game.prefConfig.enableAudioFx,
                    game.prefConfig.usbDualSenseHaptics ? game.controllerHandler.headphoneAudioSink() : null);
            game.conn.start(renderers.audio, renderers.video, game);
        }
    }

    static final class Renderers {
        final AudioRenderer audio;
        final VideoDecoderRenderer video;
        Renderers(AudioRenderer audio, VideoDecoderRenderer video) { this.audio = audio; this.video = video; }
    }

    static Renderers renderers(boolean inputOnly, boolean muteAudio, VideoDecoderRenderer decoder,
            Context context, boolean audioFx, AndroidAudioRenderer.HeadphoneSink headphones) {
        if (inputOnly) return new Renderers(new SilentAudioRenderer(), new DiscardingDecoderRenderer());
        if (muteAudio) return new Renderers(new SilentAudioRenderer(), decoder);
        return new Renderers(new AndroidAudioRenderer(context, audioFx, headphones), decoder);
    }

    void configureVideoSurface(Surface surface) {
        game.videoSurface = surface;
        game.appSurfaceFrameRateVote = null;
        game.surfaceCreated = true;
        if (game.streamRecovery.resumeStoppedStreamIfNeeded()) return;
        game.videoOutput.updateCompositorHeartbeat();
        LimeLog.info("Video surface: " + (game.prefConfig.useTextureView ? "TextureView" : "SurfaceView")
                + ", pacing: " + game.prefConfig.framePacing + ", newest frame: " + game.prefConfig.dropLateFrames);
        if (game.prefConfig.useTextureView) { game.videoOutput.logDisplayRefreshPolicy(game.videoOutput.getVideoDisplay()); return; }
        game.videoOutput.voteSurfaceFrameRate(surface);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.CINNAMON_BUN) surface.setProducerThrottlingEnabled(false);
    }

    void destroyVideoSurface() {
        if (!game.surfaceCreated) return;
        game.surfaceCreated = false;
        game.videoSurface = null;
        game.appSurfaceFrameRateVote = null;
        game.videoOutput.updateCompositorHeartbeat();
        if (game.attemptedConnection && game.decoderRenderer != null) game.decoderRenderer.prepareForStop();
        if (game.externalPresentation != null && !game.isFinishing() && !game.isDestroyed()) {
            game.videoOutput.returnVideoToDevice();
            return;
        }
        if (game.attemptedConnection) {
            game.streamRecovery.rememberStreamForResume(true);
            game.displayedFailureDialog = true;
            stopConnection();
            if (game.streamRecovery.finishesWhenStopped()) game.finish();
        }
    }
}
