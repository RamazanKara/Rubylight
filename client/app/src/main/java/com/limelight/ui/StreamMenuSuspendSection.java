package com.limelight.ui;

import android.content.Context;

import com.limelight.R;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativeQuickSuspend;
import com.limelight.nvstream.http.NvApp;
import com.limelight.ui.StreamMenu.Entry;
import com.limelight.ui.StreamMenu.Item;
import com.limelight.ui.StreamMenu.Options;
import com.limelight.ui.StreamMenu.Section;

import java.util.List;

/**
 * Quick Suspend in the stream menu: Suspend game while the host can pause this device's game, Resume game while it is
 * paused, through the native API ({@code session.quick_suspend}). The host decides who may and answers each request; a
 * refusal shows in the player's language by its reason, with the same words as desktop Nightjar
 * ({@code nightjar_policy::quick_suspend}). Reads and requests run on the stream's native menu thread, never on the
 * render, decode or input threads; their answers come back on the UI thread.
 */
public final class StreamMenuSuspendSection {
    /** What the section needs from the stream. */
    public interface Host {
        /** The host lists Quick Suspend and lets this device ask, on a stream of its own game. */
        boolean offered();

        /** The stream's native API client; null when there is none. */
        NativeApiClient client();

        /** Runs {@code task} on the native menu thread; false once the stream is over. */
        boolean background(Runnable task);

        /** Runs {@code task} on the UI thread unless the stream's screen is closing. */
        void ui(Runnable task);

        /** Shows a line over the stream. */
        void say(CharSequence text);

        /** An open stream menu shows the new state. */
        void refreshMenu();
    }

    private final Context context;
    private final Host host;
    // The host's last state other than a refusal, for the menu; null until it answers. UI thread.
    private NativeQuickSuspend state;
    private boolean busy;

    public StreamMenuSuspendSection(Context context, Host host) {
        this.context = context;
        this.host = host;
    }

    /** Adds the tile the host's last state offers, beside the host's own page. */
    static void addTo(List<Entry> entries, Options o) {
        if (o.role != NvApp.Role.STREAM || !o.quickSuspendOffered) return;
        entries.add(o.quickSuspendPaused
                ? StreamMenuCatalog.action(Section.HOST, Item.RESUME_GAME, R.string.stream_tile_resume_game, R.drawable.ic_resume)
                : StreamMenuCatalog.action(Section.HOST, Item.SUSPEND_GAME, R.string.stream_tile_suspend_game, R.drawable.ic_pause));
    }

    /** The menu offers Suspend game or Resume game. */
    public boolean offered() {
        return state != null && state.offersTile();
    }

    /** The game is paused on the PC: the menu offers Resume game. */
    public boolean paused() {
        return state != null && state.paused();
    }

    /** The menu opens: it shows the tile once the host says what it is now, as the audio switches do. */
    public void forget() {
        state = null;
    }

    /** Reads the host's state; the open menu shows the tile once it answers. */
    public void refresh() {
        if (!host.offered()) return;
        NativeApiClient client = host.client();
        if (client == null) return;
        host.background(() -> {
            NativeQuickSuspend answer = null;
            try {
                answer = client.quickSuspend();
            } catch (NativeApiException | RuntimeException e) {
                com.limelight.LimeLog.warning("Quick Suspend not read: " + e.getMessage());
            }
            NativeQuickSuspend read = answer;
            host.ui(() -> {
                if (read != null && !read.refused()) state = read;
                host.refreshMenu();
            });
        });
    }

    /** Suspend game or Resume game, chosen in the menu. */
    public void choose(Item item) {
        NativeApiClient client = host.client();
        if (busy || client == null) return;
        boolean resume = item == Item.RESUME_GAME;
        busy = true;
        boolean queued = host.background(() -> {
            NativeQuickSuspend answer = null;
            String failure = null;
            try {
                answer = resume ? client.resumeGame() : client.suspendGame();
            } catch (NativeApiException | RuntimeException e) {
                failure = e.getMessage() != null ? e.getMessage() : e.toString();
            }
            NativeQuickSuspend done = answer;
            String failed = failure;
            host.ui(() -> {
                busy = false;
                if (done != null) {
                    answered(done);
                } else {
                    com.limelight.LimeLog.warning("Quick Suspend request failed: " + failed);
                    host.say(context.getString(R.string.stream_host_action_failed, failed));
                }
            });
        });
        if (!queued) busy = false;
    }

    /** The host's answer to this device's request: a refusal leaves the game as it was and says why. */
    void answered(NativeQuickSuspend answer) {
        if (answer.refused()) {
            host.say(context.getString(refusal(answer.reason)));
            return;
        }
        boolean wasPaused = paused();
        state = answer;
        if (paused() && !wasPaused) {
            host.say(context.getString(R.string.quick_suspend_paused));
        } else if (wasPaused && NativeQuickSuspend.READY.equals(answer.state)) {
            host.say(context.getString(R.string.quick_suspend_resumed));
        }
        host.refreshMenu();
    }

    /** The words for a refusal by the host's stable reason name, as {@code nightjar_policy::quick_suspend::refusal} has them. */
    static int refusal(String reason) {
        switch (reason == null ? "" : reason) {
            case "anti_cheat_detected": return R.string.quick_suspend_refused_anti_cheat;
            case "protected_process":
            case "denylisted_process": return R.string.quick_suspend_refused_protected;
            case "guests_active":
            case "coop_session": return R.string.quick_suspend_refused_others;
            case "not_owner": return R.string.quick_suspend_refused_not_owner;
            case "session_busy": return R.string.quick_suspend_refused_busy;
            case "no_session":
            case "no_owned_process":
            case "disabled_for_app": return R.string.quick_suspend_refused_nothing;
            default: return R.string.quick_suspend_refused_failed;
        }
    }
}
