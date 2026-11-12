package com.limelight.utils;

import android.app.Activity;
import android.content.Context;
import android.content.DialogInterface;
import android.widget.Toast;

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
import com.limelight.pin.PinGate;
import com.limelight.pin.PinLockActivity;
import com.limelight.ui.GlassForm;
import com.limelight.ui.NavigationDialogBuilder;

import java.util.concurrent.atomic.AtomicReference;
import java.util.function.Consumer;

/**
 * Selecting a game another device plays: "X is playing on Y", with Continue here focused, Stream here too and
 * Cancel. A PC that does not offer the move, or that cannot say who plays, starts the stream as it always did.
 */
public final class ContinueHereGate {
    /** How long selecting a game waits to learn who plays it, before it starts the stream as it always did. */
    public static final long SELECT_READ_WAIT_MS = 1500;

    /** The press itself. */
    @VisibleForTesting
    interface Press {
        void send() throws NativeApiException;
    }

    /** One read of the PC. */
    public interface MoveRead {
        StreamMove read() throws NativeApiException, InterruptedException;
    }

    private ContinueHereGate() {
    }

    /** The answer of {@code read}, or null when it failed or took longer than {@code waitMs}. It then ends on its own. */
    public static StreamMove readWithin(MoveRead read, long waitMs) {
        AtomicReference<StreamMove> answer = new AtomicReference<>();
        Thread reading = new Thread(() -> {
            try {
                answer.set(read.read());
            } catch (NativeApiException | InterruptedException e) {
                LimeLog.warning("Could not read who plays the game; starting the stream: " + e.getMessage());
            }
        }, "Continue here read");
        reading.start();
        try {
            reading.join(waitMs);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        if (reading.isAlive()) {
            LimeLog.warning("Could not read who plays the game in time; starting the stream");
        }
        return answer.get();
    }

    /**
     * Whether this start could ask: only a plain stream of a game. A co-op seat, recovery or Input-only never does, and
     * nor does an automatic start: it has already stopped for a game elsewhere, and its page is gone by the time of an answer.
     */
    @VisibleForTesting
    static boolean plainStart(NvApp app, boolean smallStreamInputOnly, String source) {
        NvApp.Control control = app.getControl();
        return !smallStreamInputOnly && (control == NvApp.Control.NONE || reachesTheRunningGame(control))
                && app.getRole() == NvApp.Role.STREAM
                && !LaunchSource.COOP_JOIN.equals(source) && !LaunchSource.RECOVERY.equals(source)
                && !LaunchSource.OPEN_PLAY.equals(source);
    }

    /** Whether {@code move} asks about starting {@code app}. */
    @VisibleForTesting
    static boolean asks(boolean offered, boolean plainStart, StreamMove move, NvApp app) {
        return move.playingOn != null && ContinueHerePolicy.asksOnSelect(offered, plainStart,
                reachesTheRunningGame(app.getControl()) || app.matchesRunningApp(move.playingAppId, move.playingAppUuid));
    }

    /** The tiles a device beside the game gets for it: the game's own name first, then Resume. Both mean whatever runs. */
    private static boolean reachesTheRunningGame(NvApp.Control control) {
        return control == NvApp.Control.RUNNING_GAME || control == NvApp.Control.RESUME;
    }

    /**
     * Takes the start over when it may have to ask, and then calls {@code start} on the UI thread unless cancelled, with
     * the context to start from: {@code parent}, or the application's once a press outlived it.
     */
    static boolean handles(Activity parent, NvApp app, ComputerDetails computer,
                           ComputerManagerService.ComputerManagerBinder binder, boolean plainStart, Consumer<Context> start) {
        if (binder == null || !plainStart || !NativeCapabilities.offersContinueHere(KnownCapabilities.of(computer.uuid))) {
            return false;
        }
        HostEndpoint endpoint = HostEndpoint.of(computer, binder.getUniqueId(), PlatformBinding.getCryptoProvider(parent));
        if (endpoint == null) {
            return false;
        }
        NativeApiClient client = new NativeApiClient(new JniNativeApiTransport(), endpoint);
        new Thread(() -> {
            StreamMove read = readWithin(client::streamMove, SELECT_READ_WAIT_MS);
            parent.runOnUiThread(() -> {
                if (parent.isFinishing() || parent.isDestroyed()) {
                    return;
                }
                if (read != null && asks(true, true, read, app)) {
                    prompt(parent, computer, client, read, start).show();
                } else {
                    start.accept(parent);
                }
            });
        }, "Continue here select").start();
        return true;
    }

    @VisibleForTesting
    static AlertDialog prompt(Activity parent, ComputerDetails computer, NativeApiClient client, StreamMove move, Consumer<Context> start) {
        NavigationDialogBuilder builder = new NavigationDialogBuilder(parent);
        // A TV's game page has Play, Favourite and Quit game on the line where the prompt's buttons land, and a TV without
        // blur (a Shield on Android 11) shows them sharp under the dim: Quit game stood just past the panel's edge, as if the
        // panel cut it. On a TV the prompt stands on the sky instead, as the pairing PIN does.
        if (GlassForm.of(parent) == GlassForm.TELEVISION) builder.ownSurface();
        AlertDialog dialog = builder
                .setMessage(parent.getString(R.string.continue_here_playing_on, move.playingTitle, move.playingOn))
                .setPositiveButton(R.string.continue_here_action, (shown, which) -> PinLockActivity.runAfterPin(
                        parent, computer.uuid, computer.displayName(), () -> press(parent, client::continueHere, start)))
                .setNeutralButton(R.string.continue_here_stream_too, (shown, which) -> start.accept(parent))
                .setNegativeButton(android.R.string.cancel, null)
                .create();
        // A tile reached by touch leaves the dialog in touch mode, where a plain request fails and a pad's A presses nothing.
        dialog.setOnShowListener(shown -> {
            android.view.View action = dialog.getButton(DialogInterface.BUTTON_POSITIVE);
            if (!action.requestFocus()) action.requestFocusFromTouch();
        });
        return dialog;
    }

    /**
     * Moves the stream here, then starts the normal resume. A failure says so and leaves the game running. It ends
     * another device's stream, so the PIN lock comes before it, not at the start after it, and a failure leaves no grant.
     * The other device has lost its stream by the time the answer comes, so the start must happen even if this screen
     * went away meanwhile (a rotation): it goes on from the application.
     */
    @VisibleForTesting
    static void press(Activity parent, Press press, Consumer<Context> start) {
        Context application = parent.getApplicationContext();
        new Thread(() -> {
            String failure = null;
            try {
                press.send();
            } catch (NativeApiException e) {
                LimeLog.warning("Continue here failed: " + e.getMessage());
                failure = e.getMessage();
            }
            String why = failure;
            if (why != null) PinGate.forgetGrant();
            parent.runOnUiThread(() -> {
                boolean gone = parent.isFinishing() || parent.isDestroyed();
                if (why == null) {
                    start.accept(gone ? application : parent);
                } else if (gone) {
                    Toast.makeText(application, application.getString(R.string.continue_here_failed) + "\n" + why,
                            Toast.LENGTH_LONG).show();
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
