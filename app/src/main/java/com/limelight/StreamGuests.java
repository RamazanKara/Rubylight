package com.limelight;

import android.content.DialogInterface;
import android.widget.Toast;

import androidx.annotation.VisibleForTesting;
import androidx.appcompat.app.AlertDialog;

import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativeGuests;
import com.limelight.ui.NavigationDialogBuilder;

import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/**
 * Guests from other devices, on the device playing (feature session.guests): asks "X wants to join your game" with
 * Let in and Deny while the host waits for an answer, and lists the guests in the stream menu with what each may do,
 * the keyboard and mouse, Kick and Block. Reads the host every 2 s while it may manage guests, every 10 s otherwise.
 */
final class StreamGuests {
    /** How often a device that doesn't manage the guests looks again: it may become the device playing. */
    @VisibleForTesting
    static final long IDLE_POLL_MS = 10_000;

    private final Game game;
    private final NativeApiClient client;
    /** The requests already asked about, so each is asked once. */
    private final Set<Integer> asked = new HashSet<>();
    private volatile NativeGuests latest;
    private volatile Thread polling;
    private AlertDialog prompt;
    private int promptFor = -1;

    StreamGuests(Game game, NativeApiClient client) {
        this.game = game;
        this.client = client;
    }

    void start() {
        Thread thread = new Thread(this::poll, "Stream guests");
        thread.setDaemon(true);
        polling = thread;
        thread.start();
    }

    void stop() {
        Thread thread = polling;
        polling = null;
        if (thread != null) thread.interrupt();
        if (prompt != null) {
            prompt.dismiss();
            prompt = null;
        }
    }

    /** Whether this device manages the guests of its game, as the host last said. */
    boolean manages() {
        NativeGuests guests = latest;
        return guests != null && guests.manage;
    }

    private void poll() {
        Thread self = Thread.currentThread();
        while (polling == self) {
            NativeGuests read = null;
            try {
                read = client.guests();
            } catch (NativeApiException e) {
                LimeLog.info("Guests not read: " + e.getMessage());
            }
            if (read != null) show(read);
            try {
                Thread.sleep(nextPollMs(read));
            } catch (InterruptedException e) {
                return;
            }
        }
    }

    @VisibleForTesting
    static long nextPollMs(NativeGuests read) {
        return read != null && read.manage ? NativeGuests.POLL_MS : IDLE_POLL_MS;
    }

    /** The join requests to ask about now: only for the device that manages them, each once, oldest first. */
    @VisibleForTesting
    static List<NativeGuests.Request> toAsk(NativeGuests read, Set<Integer> asked) {
        List<NativeGuests.Request> ask = new ArrayList<>();
        if (read == null || !read.manage) return ask;
        for (NativeGuests.Request request : read.waiting) {
            if (!asked.contains(request.id)) ask.add(request);
        }
        return ask;
    }

    /** Whether the open prompt asks about a request the host no longer waits on (answered elsewhere, or given up). */
    @VisibleForTesting
    static boolean stale(NativeGuests read, int promptFor) {
        if (promptFor < 0) return false;
        if (read == null || !read.manage) return true;
        for (NativeGuests.Request request : read.waiting) {
            if (request.id == promptFor) return false;
        }
        return true;
    }

    private void show(NativeGuests read) {
        game.runOnUiThread(() -> {
            if (polling == null || game.isFinishing()) return;
            latest = read;
            if (prompt != null && stale(read, promptFor)) {
                prompt.dismiss();
                prompt = null;
                promptFor = -1;
            }
            if (prompt != null) return;
            List<NativeGuests.Request> ask = toAsk(read, asked);
            if (!ask.isEmpty()) askAbout(ask.get(0));
        });
    }

    private void askAbout(NativeGuests.Request request) {
        asked.add(request.id);
        promptFor = request.id;
        prompt = new NavigationDialogBuilder(game)
                .setMessage(game.getString(R.string.guests_wants_to_join, request.name))
                .setPositiveButton(R.string.guests_let_in, (dialog, which) -> act("approve", request.id, null))
                .setNegativeButton(R.string.guests_deny, (dialog, which) -> act("deny", request.id, null))
                .create();
        prompt.setOnDismissListener(dialog -> {
            if (prompt == dialog) {
                prompt = null;
                promptFor = -1;
            }
        });
        prompt.setOnShowListener(shown -> {
            android.view.View action = prompt.getButton(DialogInterface.BUTTON_POSITIVE);
            if (action != null && !action.requestFocus()) action.requestFocusFromTouch();
        });
        prompt.show();
    }

    /** The stream menu's Guests: each guest with what it may do; selecting one offers the changes. */
    void showGuests() {
        NativeGuests read = latest;
        if (read == null || game.isFinishing()) return;
        NavigationDialogBuilder builder = new NavigationDialogBuilder(game);
        builder.setTitle(R.string.guests_title);
        if (read.guests.isEmpty()) {
            builder.setMessage(R.string.guests_none).setPositiveButton(android.R.string.ok, null).show();
            return;
        }
        String[] lines = new String[read.guests.size()];
        for (int i = 0; i < lines.length; i++) lines[i] = line(read, read.guests.get(i));
        builder.setItems(lines, (dialog, which) -> showGuest(read, read.guests.get(which)))
                .setNegativeButton(android.R.string.cancel, null)
                .show();
    }

    private String line(NativeGuests read, NativeGuests.Guest guest) {
        StringBuilder line = new StringBuilder(guest.name).append(" · ").append(permissionLabel(guest.permission));
        if (read.keyboardMouse == guest.id) line.append(" · ").append(game.getString(R.string.guests_has_keyboard));
        if (!guest.streaming) line.append(" · ").append(game.getString(R.string.guests_away));
        return line.toString();
    }

    private String permissionLabel(String permission) {
        if (NativeGuests.VIEW.equals(permission)) return game.getString(R.string.guests_view);
        if (NativeGuests.KEYBOARD_MOUSE.equals(permission)) return game.getString(R.string.guests_keyboard_mouse);
        return game.getString(R.string.guests_controller);
    }

    /** The changes offered for one guest, in the order shown. */
    @VisibleForTesting
    enum Change { VIEW, CONTROLLER, KEYBOARD_MOUSE, GIVE_KEYBOARD, TAKE_KEYBOARD, KICK, BLOCK }

    @VisibleForTesting
    static List<Change> changes(NativeGuests read, NativeGuests.Guest guest) {
        List<Change> changes = new ArrayList<>();
        if (!NativeGuests.VIEW.equals(guest.permission)) changes.add(Change.VIEW);
        if (!NativeGuests.CONTROLLER.equals(guest.permission)) changes.add(Change.CONTROLLER);
        if (!NativeGuests.KEYBOARD_MOUSE.equals(guest.permission)) changes.add(Change.KEYBOARD_MOUSE);
        if (NativeGuests.usesKeyboardMouse(guest.permission) && guest.streaming) {
            changes.add(read.keyboardMouse == guest.id ? Change.TAKE_KEYBOARD : Change.GIVE_KEYBOARD);
        }
        changes.add(Change.KICK);
        changes.add(Change.BLOCK);
        return changes;
    }

    private void showGuest(NativeGuests read, NativeGuests.Guest guest) {
        List<Change> changes = changes(read, guest);
        String[] labels = new String[changes.size()];
        for (int i = 0; i < labels.length; i++) labels[i] = label(changes.get(i));
        new NavigationDialogBuilder(game)
                .setTitle(guest.name)
                .setItems(labels, (dialog, which) -> apply(changes.get(which), guest))
                .setNegativeButton(android.R.string.cancel, null)
                .show();
    }

    private String label(Change change) {
        switch (change) {
            case VIEW: return game.getString(R.string.guests_view);
            case CONTROLLER: return game.getString(R.string.guests_controller);
            case KEYBOARD_MOUSE: return game.getString(R.string.guests_keyboard_mouse);
            case GIVE_KEYBOARD: return game.getString(R.string.guests_give_keyboard);
            case TAKE_KEYBOARD: return game.getString(R.string.guests_take_keyboard);
            case KICK: return game.getString(R.string.guests_kick);
            default: return game.getString(R.string.guests_block);
        }
    }

    private void apply(Change change, NativeGuests.Guest guest) {
        switch (change) {
            case VIEW: act("permission", guest.id, NativeGuests.VIEW); break;
            case CONTROLLER: act("permission", guest.id, NativeGuests.CONTROLLER); break;
            case KEYBOARD_MOUSE: act("permission", guest.id, NativeGuests.KEYBOARD_MOUSE); break;
            case GIVE_KEYBOARD: act("keyboard_mouse", guest.id, null); break;
            case TAKE_KEYBOARD: act("keyboard_mouse", -1, null); break;
            case KICK: act("kick", guest.id, null); break;
            default: act("revoke", guest.id, null); break;
        }
    }

    /** Sends one action off the UI thread; a refusal is shown in the host's words. */
    private void act(String action, int id, String permission) {
        new Thread(() -> {
            try {
                NativeGuests after = client.guestAction(action, id, permission);
                game.runOnUiThread(() -> latest = after);
            } catch (NativeApiException e) {
                LimeLog.warning("Guest " + action + " failed: " + e.getMessage());
                game.runOnUiThread(() -> {
                    if (game.isFinishing()) return;
                    Toast.makeText(game, game.getString(R.string.guests_failed) + "\n" + e.getMessage(), Toast.LENGTH_LONG).show();
                });
            }
        }, "Guest action").start();
    }
}
