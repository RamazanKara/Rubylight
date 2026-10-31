package com.limelight;

import com.limelight.binding.video.PackedFallback;
import com.limelight.binding.video.MediaCodecDecoderRenderer;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.utils.LaunchSource;
import com.limelight.utils.SpinnerDialog;
import com.limelight.ui.NavigationDialogBuilder;
import android.content.Intent;
import android.os.SystemClock;
import android.widget.Toast;
import java.io.IOException;

/** Getting a dropped or stopped stream back: automatic recovery and reconnects, the trip budget, resuming a stopped stream, the packed-chroma fallback, the route to the host and the reconnect dialog. */
final class GameRecovery {
    private final Game game;

    GameRecovery(Game game) {
        this.game = game;
    }

    private long connectedAt = -1;
    // How long the stream had been connected when the user's trip began, carried by its resume.
    private long connectedBeforeTrip;

    // Volatile: a recovery's route wait on its worker stops when this changes.
    private volatile int recoveryGeneration;

    // The replacement stream a queued recovery will start; a packed-chroma failure meanwhile marks it.
    private Intent pendingRecoveryIntent;

    private static final String EXTRA_OUTPUT_RESUME_ONLY = "rubylight.outputResumeOnly";

    private static final String EXTRA_RECOVERY_ATTEMPTS = "RecoveryAttempts";

    private static final String EXTRA_RECOVERY_STARTED = "RecoveryStarted";

    // An earlier connection of this automatic reconnect chain showed video (videoStarted).
    private static final String EXTRA_RECOVERY_VIDEO = "RecoveryVideoShown";

    // On a resume after the user's own trip away: the failure budget {attempts, started, left at,
    // connected for} as the trip began, which the resumed activity starts with.
    private static final String EXTRA_TRIP_BUDGET = "rubylight.tripBudget";

    void rememberMicrophoneMute() {
        if (game.conn != null && (game.connected || game.connecting)) {
            game.getIntent().putExtra(Game.EXTRA_MICROPHONE_MUTED, game.conn.isMicrophoneMutedForReconnect());
        }
    }

    void restoreMicrophoneMute() {
        // This is session state, applied before controller binding or native capture can start.
        game.conn.setMicrophoneMuted(game.getIntent().getBooleanExtra(Game.EXTRA_MICROPHONE_MUTED, false));
    }

    /** The stream started again, {@code why} (a {@link LaunchSource}); the log keeps the start that opened it. */
    private Intent createStreamHandoffIntent(String why) {
        rememberMicrophoneMute();
        Intent again = LaunchSource.put(new Intent(game.getIntent()), LaunchSource.again(why, game.getIntent()));
        // The game this start replaced may be long gone: another device's could be running by now.
        again.removeExtra(Game.EXTRA_REPLACED_APP_NAME);
        // Only a resume after the user's trip carries them; any other start spends the budget it has.
        again.removeExtra(EXTRA_TRIP_BUDGET);
        again.removeExtra(Game.EXTRA_TRIP_RESUME_STARTED);
        return again;
    }

    private Intent createReconnectIntent() {
        Intent retry = createStreamHandoffIntent(LaunchSource.RECONNECT);
        retry.removeExtra(EXTRA_RECOVERY_ATTEMPTS);
        retry.removeExtra(EXTRA_RECOVERY_STARTED);
        retry.removeExtra(Game.EXTRA_RECOVERY_MEDIA_FAILURE);
        retry.removeExtra(EXTRA_RECOVERY_VIDEO);
        retry.removeExtra(EXTRA_OUTPUT_RESUME_ONLY);
        if (game.outputSwitchPending) {
            if (game.app == null || game.app.isControlAction()) return null;
            if (game.app.getRole() == NvApp.Role.STREAM) {
                // Output changes only move the existing stream. Resolve Resume/Running
                // Game tiles to the actual game so a changed host session is rejected
                // by NvConnection's existing resume-only checks, never launched/quit.
                int resumeId = game.recoveryAppId != 0 ? game.recoveryAppId :
                        game.app.getControl() == NvApp.Control.NONE ? game.app.getAppId() : 0;
                if (resumeId == 0) return null;
                retry.putExtra(Game.EXTRA_APP_ID, resumeId)
                        .putExtra(Game.EXTRA_APP_UUID, game.recoveryAppId != 0 ? game.recoveryAppUuid : game.app.getAppUuid())
                        .putExtra(EXTRA_OUTPUT_RESUME_ONLY, true);
            }
            // Remote Monitor retains its dedicated view-only host operation.
        }
        return retry;
    }

    void reconnectStream() {
        if (game.recoveryHandoffInProgress || game.isFinishing() || game.isDestroyed()) return;
        game.stoppedStreamResumeIntent = null;
        final Intent retry = createReconnectIntent();
        if (retry == null) {
            game.stopConnection();
            game.finish();
            return;
        }
        game.displayedFailureDialog = true;
        game.touchInput.setInputGrabState(false);
        game.stopConnection();
        retry.putExtra(Game.EXTRA_MICROPHONE_MUTED, game.getIntent().getBooleanExtra(Game.EXTRA_MICROPHONE_MUTED, false));
        final Thread stopping = game.connectionStopThread;
        final int generation = game.foregroundGeneration;
        final int recovery = recoveryGeneration;
        game.spinner = SpinnerDialog.displayDialog(game, game.getString(R.string.conn_establishing_title),
                game.getString(R.string.stream_reconnecting), true);
        game.recoveryHandoffInProgress = true;
        new Thread(() -> {
            if (stopping != null) {
                try {
                    stopping.join();
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    return;
                }
            }
            game.runOnUiThread(() -> {
                if (game.foreground && generation == game.foregroundGeneration && recovery == recoveryGeneration &&
                        !game.isFinishing() && !game.isDestroyed()) {
                    game.recoveryHandoffInProgress = false;
                    game.finishNow();
                    game.startActivity(retry);
                }
            });
        }, "Stream reconnect").start();
    }

    boolean isRecoveryAttempt() {
        return game.getIntent().getIntExtra(EXTRA_RECOVERY_ATTEMPTS, 0) > 0 || isTripResume();
    }

    boolean isResumeOnlyStream() {
        return isRecoveryAttempt() || game.getIntent().getBooleanExtra(EXTRA_OUTPUT_RESUME_ONLY, false);
    }

    void cancelAutomaticRecovery() {
        recoveryGeneration++;
        game.recoveryHandoffInProgress = false;
        if (game.pendingRecovery != null) game.bitrateHandler.removeCallbacks(game.pendingRecovery);
        game.pendingRecovery = null;
        pendingRecoveryIntent = null;
        game.bitrateHandler.removeCallbacks(game.recoveryTimeout);
    }

    /** {@code userTrip}: the user left (Home, Files, a lock), so no connection failure caused this stop. */
    void rememberStreamForResume(boolean userTrip) {
        if (game.stoppedStreamResumeIntent != null || game.prefConfig == null || !game.prefConfig.autoReconnect ||
                !game.connected || game.displayedFailureDialog || game.isFinishing() || game.isDestroyed() ||
                game.recoveryCancelledOnPause || game.recoveryHandoffInProgress || game.recoveryAppId == 0 ||
                game.app == null || game.app.getRole() != NvApp.Role.STREAM || game.app.isControlAction()) return;
        // Count only time actually connected, never time spent locked or in another app.
        long now = SystemClock.elapsedRealtime();
        game.recoveryPolicy.resetAfterStableConnection(now, connectedAt);
        long connectedFor = connectedAt >= 0 ? now - connectedAt : 0;
        connectedAt = -1;
        game.stoppedStreamResumeIntent = createStreamHandoffIntent(LaunchSource.RETURN)
                .putExtra(Game.EXTRA_APP_ID, game.recoveryAppId).putExtra(Game.EXTRA_APP_UUID, game.recoveryAppUuid);
        // A failure already reported as the user left is a failure: its resume spends the budget.
        if (userTrip && !game.streamTerminated) {
            game.stoppedStreamResumeIntent.putExtra(EXTRA_TRIP_BUDGET,
                    new long[] {game.recoveryPolicy.attempts(), game.recoveryPolicy.startedAt(), now, connectedFor});
        }
    }

    boolean isTripResume() {
        return game.getIntent().hasExtra(Game.EXTRA_TRIP_RESUME_STARTED);
    }

    /**
     * The failure budget this activity starts with (onCreate). A resume after the user's trip takes
     * the budget recorded as the trip began, the time since leaving not counted, so a failure while
     * it starts up continues that budget; any other start takes the one its intent carries. The
     * time connected before the trip still counts toward the 30 s that earns a new budget.
     */
    void readRecoveryBudget(long now) {
        long[] trip = game.getIntent().getLongArrayExtra(EXTRA_TRIP_BUDGET);
        if (trip == null) {
            game.recoveryPolicy = new StreamRecoveryPolicy(game.getIntent().getIntExtra(EXTRA_RECOVERY_ATTEMPTS, 0),
                    game.getIntent().getLongExtra(EXTRA_RECOVERY_STARTED, 0));
            return;
        }
        game.recoveryPolicy = StreamRecoveryPolicy.afterUserTrip((int) trip[0], trip[1], trip[2], now);
        connectedBeforeTrip = game.recoveryPolicy.attempts() > 0 ? trip[3] : 0;
        game.getIntent().removeExtra(EXTRA_TRIP_BUDGET);
        game.getIntent().putExtra(EXTRA_RECOVERY_ATTEMPTS, game.recoveryPolicy.attempts())
                .putExtra(EXTRA_RECOVERY_STARTED, game.recoveryPolicy.startedAt());
    }

    /**
     * The stream connected (connectionStarted): stability counts from here, after the time connected
     * before a trip, and the startup deadline ends.
     */
    void onStreamConnected(long now) {
        connectedAt = now - connectedBeforeTrip;
        game.bitrateHandler.removeCallbacks(game.recoveryTimeout);
    }

    // Without stream recovery, leaving the stream (Home, closing PiP) ends it, as it always did.
    // With recovery on, the activity may stay to resume the same game or to offer Reconnect.
    boolean finishesWhenStopped() {
        return game.prefConfig == null || !game.prefConfig.autoReconnect ||
                (game.stoppedStreamResumeIntent == null && !game.recoveryCancelledOnPause);
    }

    boolean resumeStoppedStreamIfNeeded() {
        if (game.stoppedStreamResumeIntent == null || !game.foreground || game.isFinishing() || game.isDestroyed()) return false;
        Intent retry = game.stoppedStreamResumeIntent;
        game.stoppedStreamResumeIntent = null;
        // After the user's own trip the resume skips the failure budget; after a failure it spends it.
        if (!queueAutomaticRecovery(retry, false, retry.hasExtra(EXTRA_TRIP_BUDGET))) {
            game.displayedFailureDialog = true;
            if (game.spinner != null) { game.spinner.dismiss(); game.spinner = null; }
            showReconnectDialog(R.string.conn_terminated_title, game.getString(R.string.stream_recovery_exhausted));
        }
        return true;
    }

    boolean tryAutomaticRecovery() {
        return tryAutomaticRecovery(false);
    }

    /**
     * Packed full chroma stopped working: reconnect once without it for the rest of the session,
     * whether or not automatic reconnection is on (this is not a lost connection). False when this
     * session already runs without it or cannot reconnect now; the caller then ends the stream.
     */
    boolean reconnectWithoutPackedChroma(String reason) {
        return reconnectWithoutPackedChroma("Packed full chroma stopped: " + reason, PackedFallback.toast());
    }

    boolean reconnectWithoutPackedChroma(String logLine, String toast) {
        boolean unpacked = game.getIntent().getBooleanExtra(Game.EXTRA_PACKED_OFF, false);
        boolean canReconnect = game.foreground && !game.isFinishing() && !game.isDestroyed();
        if (PackedFallback.onFailure(unpacked, canReconnect) != PackedFallback.Action.RECONNECT_WITHOUT_PACKED) return false;
        LimeLog.warning(logLine);
        if (!tryAutomaticRecovery(true)) return false;
        Toast.makeText(game.getApplicationContext(), toast, Toast.LENGTH_LONG).show();
        return true;
    }

    // Runs on the render thread: only decides on the UI thread; the stream ends from a worker if it cannot reconnect.
    void onPackedChromaFailure(String reason) {
        game.runOnUiThread(() -> {
            if (game.isFinishing() || game.isDestroyed() || reconnectWithoutPackedChroma(reason)) return;
            // Whatever happens next (a queued recovery, a resume, the Reconnect dialog), copies of this
            // activity's intent keep the mark, so no later reconnect asks for packed chroma again.
            game.getIntent().putExtra(Game.EXTRA_PACKED_OFF, true);
            if ((game.pendingRecovery != null || game.recoveryHandoffInProgress) && pendingRecoveryIntent != null) {
                // A recovery is already on its way (the failure came with a lost connection): its
                // replacement stream starts without packed chroma instead of a dialog over it.
                LimeLog.warning("Packed full chroma stopped during a recovery: " + reason);
                pendingRecoveryIntent.putExtra(Game.EXTRA_PACKED_OFF, true);
                return;
            }
            if (game.stoppedStreamResumeIntent != null) {
                // Left in the background and kept for resuming: it resumes without packed chroma.
                LimeLog.warning("Packed full chroma stopped while the stream waits to resume: " + reason);
                game.stoppedStreamResumeIntent.putExtra(Game.EXTRA_PACKED_OFF, true);
                return;
            }
            if (game.displayedFailureDialog) return;
            endStreamAfterPackedFailure(reason);
        });
    }

    // connectionTerminated tests the network and asks the PC (blocking calls), so never on the UI thread.
    private void endStreamAfterPackedFailure(String reason) {
        new Thread(() -> {
            MoonBridge.bridgeClDisplayMessage("Packed full-chroma presentation stopped: " + reason);
            MoonBridge.bridgeClConnectionTerminated(-1);
        }, "Packed chroma end").start();
    }

    boolean tryAutomaticRecovery(boolean packedOff) {
        return tryAutomaticRecovery(packedOff, false);
    }

    /** {@code immediate}: no delay before the attempt (it still spends one); a network came back. */
    boolean tryAutomaticRecovery(boolean packedOff, boolean immediate) {
        if (game.prefConfig == null || (!packedOff && !game.prefConfig.autoReconnect) || game.displayedFailureDialog ||
                (!game.connected && !game.connecting) || !game.foreground || game.isFinishing() || game.isDestroyed() ||
                game.app == null || game.app.getRole() != NvApp.Role.STREAM || game.app.isControlAction()) return false;
        int resumeId = game.recoveryAppId != 0 ? game.recoveryAppId :
                game.app.getControl() == NvApp.Control.NONE ? game.app.getAppId() : 0;
        String resumeUuid = game.recoveryAppId != 0 ? game.recoveryAppUuid : game.app.getAppUuid();
        if (resumeId == 0 && packedOff) {
            // Full chroma refused before connectionStarted resolved a Resume or Running Game tile:
            // resume the game the PC reported running, as connectionStarted would, else repeat the tile.
            ComputerDetails host = game.conn != null ? game.conn.getHostDetails() : null;
            if (host != null && host.runningGameId != 0) {
                resumeId = host.runningGameId;
                resumeUuid = host.runningGameUuid;
            } else {
                resumeId = game.app.getAppId();
            }
        }
        if (resumeId == 0) return false;
        Intent retry = createStreamHandoffIntent(LaunchSource.RECOVERY)
                .putExtra(Game.EXTRA_APP_ID, resumeId).putExtra(Game.EXTRA_APP_UUID, resumeUuid);
        if (packedOff) retry.putExtra(Game.EXTRA_PACKED_OFF, true);
        // Only a drop after video marks the chain; a restart before it (packed chroma) carries what it had.
        if (videoStarted()) retry.putExtra(EXTRA_RECOVERY_VIDEO, true);
        return queueAutomaticRecovery(retry, packedOff, false, immediate);
    }

    /**
     * Whether video started: this connection showed a frame, or an earlier one of its automatic
     * reconnect chain did. A reconnect after a real drop that has no frame yet when it drops again
     * keeps reconnecting within the budget; a stream that never had video does not reconnect
     * (Moonlight V+ PC 6792aeed).
     */
    boolean videoStarted() {
        MediaCodecDecoderRenderer videoRenderer = game.decoderRenderer;
        return (videoRenderer != null && videoRenderer.hasReceivedVideo()) ||
                game.getIntent().getBooleanExtra(EXTRA_RECOVERY_VIDEO, false);
    }

    /**
     * Whether a lost connection would reconnect on its own now: what tryAutomaticRecovery
     * checks, and an attempt left in the budget. Spends nothing.
     */
    boolean recoversAutomatically() {
        if (game.prefConfig == null || !game.prefConfig.autoReconnect || game.displayedFailureDialog || !game.connected || !game.foreground ||
                !videoStarted() ||
                game.isFinishing() || game.isDestroyed() || game.recoveryHandoffInProgress || game.pendingRecovery != null ||
                game.app == null || game.app.getRole() != NvApp.Role.STREAM || game.app.isControlAction()) return false;
        int resumeId = game.recoveryAppId != 0 ? game.recoveryAppId :
                game.app.getControl() == NvApp.Control.NONE ? game.app.getAppId() : 0;
        return resumeId != 0 && game.recoveryPolicy.allowsAttempt(SystemClock.elapsedRealtime(), connectedAt);
    }

    private boolean queueAutomaticRecovery(final Intent retry, boolean ignoreAutoReconnectSetting) {
        return queueAutomaticRecovery(retry, ignoreAutoReconnectSetting, false);
    }

    private boolean queueAutomaticRecovery(final Intent retry, boolean ignoreAutoReconnectSetting, boolean tripResume) {
        return queueAutomaticRecovery(retry, ignoreAutoReconnectSetting, tripResume, false);
    }

    /**
     * {@code tripResume}: the resume after the user's own trip. It is never counted or refused by
     * the failure budget: a fixed delay and a deadline of its own from the return.
     * {@code immediate}: no delay. Every recovery waits for a route to the PC before it starts.
     */
    private boolean queueAutomaticRecovery(final Intent retry, boolean ignoreAutoReconnectSetting, boolean tripResume,
                                           boolean immediate) {
        if (game.prefConfig == null || (!ignoreAutoReconnectSetting && !game.prefConfig.autoReconnect) || !game.foreground || game.isFinishing() || game.isDestroyed() ||
                game.recoveryHandoffInProgress || game.pendingRecovery != null) return false;
        long now = SystemClock.elapsedRealtime();
        long delay;
        final long deadlineFrom;
        if (tripResume) {
            delay = StreamRecoveryPolicy.TRIP_RESUME_DELAY_MS;
            deadlineFrom = now;
            retry.putExtra(Game.EXTRA_TRIP_RESUME_STARTED, now);
        } else {
            delay = game.recoveryPolicy.nextDelay(now, connectedAt);
            if (delay < 0) {
                game.recoveryBudgetExhausted = true;
                return false;
            }
            if (immediate) delay = 0;
            deadlineFrom = game.recoveryPolicy.startedAt();
            retry.putExtra(EXTRA_RECOVERY_ATTEMPTS, game.recoveryPolicy.attempts())
                    .putExtra(EXTRA_RECOVERY_STARTED, game.recoveryPolicy.startedAt());
        }
        final String routeHost = game.getIntent().getStringExtra(Game.EXTRA_HOST);
        final int routePort = game.getIntent().getIntExtra(Game.EXTRA_PORT, NvHTTP.DEFAULT_HTTP_PORT);

        game.displayedFailureDialog = true;
        game.touchInput.setInputGrabState(false);
        game.stopConnection();
        if (game.spinner != null) game.spinner.dismiss();
        game.spinner = SpinnerDialog.displayDialog(game, game.getString(R.string.conn_establishing_title), tripResume ?
                game.getString(R.string.stream_reconnecting) :
                game.getString(R.string.stream_recovery_attempt, game.recoveryPolicy.attempts(), StreamRecoveryPolicy.MAX_ATTEMPTS), true);
        retry.putExtra(Game.EXTRA_MICROPHONE_MUTED, game.getIntent().getBooleanExtra(Game.EXTRA_MICROPHONE_MUTED, false));
        pendingRecoveryIntent = retry;
        final int generation = game.foregroundGeneration;
        final int recovery = recoveryGeneration;
        // Keep this true after the delayed callback hands off to its teardown worker. A
        // pause/resume in that interval must still replace the spinner with manual recovery.
        game.recoveryHandoffInProgress = true;
        game.pendingRecovery = () -> {
            game.pendingRecovery = null;
            if (!game.foreground || generation != game.foregroundGeneration || recovery != recoveryGeneration || game.isFinishing() || game.isDestroyed()) return;
            final Thread stopping = game.connectionStopThread;
            new Thread(() -> {
                try {
                    // Never overlap the native global connection with a previous stop.
                    if (stopping != null) stopping.join(10000);
                    waitForRouteToHost(routeHost, routePort, deadlineFrom + StreamRecoveryPolicy.WINDOW_MS, recovery);
                } catch (InterruptedException interrupted) {
                    Thread.currentThread().interrupt();
                    return;
                }
                game.runOnUiThread(() -> {
                    if (!game.foreground || generation != game.foregroundGeneration || recovery != recoveryGeneration || game.isFinishing() || game.isDestroyed()) return;
                    if ((stopping != null && stopping.isAlive()) ||
                            SystemClock.elapsedRealtime() - deadlineFrom >= StreamRecoveryPolicy.WINDOW_MS) {
                        game.recoveryHandoffInProgress = false;
                        if (game.spinner != null) { game.spinner.dismiss(); game.spinner = null; }
                        showReconnectDialog(R.string.conn_terminated_title, game.getString(R.string.stream_recovery_exhausted));
                        return;
                    }
                    game.recoveryHandoffInProgress = false;
                    game.finishNow();
                    game.startActivity(retry);
                });
            }, "Automatic stream recovery").start();
        };
        game.bitrateHandler.postDelayed(game.pendingRecovery, delay);
        return true;
    }

    /**
     * On a recovery's worker: returns once the PC is reachable, at {@code deadline}, or when the
     * recovery is cancelled. A network is often back before its address toward the PC (DHCP),
     * so the attempt waits for a route instead of failing at once and backing off.
     */
    private void waitForRouteToHost(String host, int port, long deadline, int recovery) throws InterruptedException {
        if (host == null) return;
        NetworkReturnRecovery wait = new NetworkReturnRecovery(deadline);
        long waitStarted = SystemClock.elapsedRealtime();
        while (recovery == recoveryGeneration && !game.isFinishing() && !game.isDestroyed()) {
            long checkedAt = SystemClock.elapsedRealtime();
            String unreachable = routeProbe.problem(host, port);
            long next = wait.next(SystemClock.elapsedRealtime(), unreachable == null);
            if (next == 0) {
                if (wait.unreachableChecks() > 0) {
                    LimeLog.info("Recovery: the PC is reachable again after " +
                            (SystemClock.elapsedRealtime() - waitStarted) + " ms");
                }
                return;
            }
            if (next < 0) {
                LimeLog.warning("Recovery: the PC stayed unreachable until the recovery window ended: " + unreachable);
                return;
            }
            if (wait.unreachableChecks() == 1) {
                LimeLog.info("Recovery: no route to the PC yet (" + unreachable + "), checking every " +
                        NetworkReturnRecovery.RETRY_MS + " ms");
            }
            Thread.sleep(Math.max(0, next - (SystemClock.elapsedRealtime() - checkedAt)));
        }
    }

    // A recovery's route check toward the PC (tests replace it).
    private Game.RouteProbe routeProbe = GameRecovery::routeProblem;

    /**
     * Why there is no route toward the PC's HTTP port yet, or null to go on with the attempt: an
     * answer, refused included, and also a check that timed out or failed otherwise (a slow link
     * is not a missing one; the attempt itself decides, loudly).
     */
    private static String routeProblem(String host, int port) {
        try (java.net.Socket socket = new java.net.Socket()) {
            socket.connect(new java.net.InetSocketAddress(host, port), NetworkReturnRecovery.PROBE_TIMEOUT_MS);
            return null;
        } catch (java.io.IOException e) {
            return Game.routeMissing(e) ? e.toString() : null;
        }
    }

    void showReconnectDialog(int title, String message) {
        game.menuActions.showStreamDialog(new NavigationDialogBuilder(game).setTitle(title).setMessage(message)
                .setCancelable(false)
                .setNegativeButton(R.string.stream_disconnect, (dialog, which) -> game.finish())
                .setPositiveButton(R.string.stream_reconnect, (dialog, which) -> reconnectStream()).create());
    }
}
