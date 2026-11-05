package com.limelight.utils;

import android.app.Activity;
import android.content.DialogInterface;

import androidx.annotation.VisibleForTesting;
import androidx.appcompat.app.AlertDialog;

import com.limelight.ContinueHerePolicy;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.PlatformBinding;
import com.limelight.computers.ComputerManagerService;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.JniNativeApiTransport;
import com.limelight.nativeapi.KnownCapabilities;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativeCapabilities;
import com.limelight.nativeapi.StreamMove;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.ui.NavigationDialogBuilder;

/**
 * Selecting a game another device plays: "X is playing on Y", with Continue here focused, Stream here too and
 * Cancel. A PC that does not offer the move, or that cannot say who plays, starts the stream as it always did.
 */
final class ContinueHereGate {
    private ContinueHereGate() {
    }

    /** Whether this start could ask: only a plain stream of a game. A co-op seat, recovery or Input-only never does. */
    @VisibleForTesting
    static boolean plainStart(NvApp app, boolean smallStreamInputOnly, String source) {
        return !smallStreamInputOnly && app.getControl() == NvApp.Control.NONE && app.getRole() == NvApp.Role.STREAM
                && !LaunchSource.COOP_JOIN.equals(source) && !LaunchSource.RECOVERY.equals(source);
    }

    /** Whether {@code move} asks about starting {@code app}. */
    @VisibleForTesting
    static boolean asks(boolean offered, boolean plainStart, StreamMove move, NvApp app) {
        return move.playingOn != null && ContinueHerePolicy.asksOnSelect(offered, plainStart,
                app.matchesRunningApp(move.playingAppId, move.playingAppUuid));
    }

    /** Takes the start over when it may have to ask, and then calls {@code start} on the UI thread unless cancelled. */
    static boolean handles(Activity parent, NvApp app, ComputerDetails computer,
                           ComputerManagerService.ComputerManagerBinder binder, boolean plainStart, Runnable start) {
        if (binder == null || !plainStart || !NativeCapabilities.offersContinueHere(KnownCapabilities.of(computer.uuid))) {
            return false;
        }
        HostEndpoint endpoint = HostEndpoint.of(computer, binder.getUniqueId(), PlatformBinding.getCryptoProvider(parent));
        if (endpoint == null) {
            return false;
        }
        NativeApiClient client = new NativeApiClient(new JniNativeApiTransport(), endpoint);
        new Thread(() -> {
            StreamMove move = null;
            try {
                move = client.streamMove();
            } catch (NativeApiException e) {
                LimeLog.warning("Could not read who plays the game; starting the stream: " + e.getMessage());
            }
            StreamMove read = move;
            parent.runOnUiThread(() -> {
                if (parent.isFinishing() || parent.isDestroyed()) {
                    return;
                }
                if (read != null && asks(true, true, read, app)) {
                    prompt(parent, client, read, start).show();
                } else {
                    start.run();
                }
            });
        }, "Continue here read").start();
        return true;
    }

    private static AlertDialog prompt(Activity parent, NativeApiClient client, StreamMove move, Runnable start) {
        AlertDialog dialog = new NavigationDialogBuilder(parent)
                .setMessage(parent.getString(R.string.continue_here_playing_on, move.playingTitle, move.playingOn))
                .setPositiveButton(R.string.continue_here_action, (shown, which) -> press(parent, client, start))
                .setNeutralButton(R.string.continue_here_stream_too, (shown, which) -> start.run())
                .setNegativeButton(android.R.string.cancel, null)
                .create();
        dialog.setOnShowListener(shown -> dialog.getButton(DialogInterface.BUTTON_POSITIVE).requestFocus());
        return dialog;
    }

    /** Moves the stream here, then starts the normal resume. A failure says so and leaves the game running. */
    private static void press(Activity parent, NativeApiClient client, Runnable start) {
        new Thread(() -> {
            String failure = null;
            try {
                client.continueHere();
            } catch (NativeApiException e) {
                LimeLog.warning("Continue here failed: " + e.getMessage());
                failure = e.getMessage();
            }
            String why = failure;
            parent.runOnUiThread(() -> {
                if (parent.isFinishing() || parent.isDestroyed()) {
                    return;
                }
                if (why == null) {
                    start.run();
                } else {
                    new NavigationDialogBuilder(parent)
                            .setTitle(R.string.continue_here_failed)
                            .setMessage(why)
                            .setPositiveButton(android.R.string.ok, null)
                            .show();
                }
            });
        }, "Continue here").start();
    }
}
