package com.limelight.computers;

import android.app.Activity;
import android.content.Context;

import androidx.annotation.VisibleForTesting;

import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativeCoop;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.ui.ActionSheet;

import com.google.android.material.button.MaterialButton;

import java.util.concurrent.Executor;

/**
 * Join co-op (Experimental): the PC's running Nucleus Co-op sessions, each with its players, its free slots and this
 * device's own seat, and a join that seats this device and then starts its stream the normal way. POST /coop/join holds
 * the slot for two minutes, until the launch of the session's co-op entry streams it.
 *
 * <p>Host calls run on {@code worker}; their answers reach the sheet on {@code main}, and only the answer to the
 * newest call while the sheet is open: one that arrives later is logged and dropped, so a closed sheet never starts a
 * stream. Every failure is a dialog with the host's words.
 */
final class CoopSheet {
    interface Client {
        /** The paired host's native API client; throws when it can't be reached that way. */
        NativeApiClient open() throws NativeApiException;
    }

    interface Ui {
        /** A failure: its title, the host's words, and Try again when doing it again can help (else null). */
        void problem(CharSequence title, CharSequence message, Runnable retry);

        /** Joined: start the stream of {@code app}, the session's co-op entry. */
        void start(NvApp app);

        /** Opens the sheet again, after the list could not be read. */
        void reopen();
    }

    private final Activity activity;
    private final ComputerDetails computer;
    private final Client client;
    private final Executor worker;
    private final Executor main;
    private final Ui ui;
    private final ActionSheet sheet;
    // Main thread only. Each host call takes the next number; an answer to an older one is dropped.
    private int request;
    private boolean closed;
    private boolean joining;
    private NativeCoop shown;

    CoopSheet(Activity activity, ComputerDetails computer, Client client, Executor worker, Executor main, Ui ui) {
        this.activity = activity;
        this.computer = computer;
        this.client = client;
        this.worker = worker;
        this.main = main;
        this.ui = ui;
        sheet = new ActionSheet(activity, activity.getString(R.string.coop_title, computer.displayName()));
        sheet.setOnClosed(() -> closed = true);
    }

    @VisibleForTesting
    ActionSheet sheet() {
        return sheet;
    }

    void show() {
        sheet.show();
        load();
    }

    /** Closes the sheet; what the host answers later is dropped. */
    void close() {
        closed = true;
        sheet.dismiss();
    }

    private void load() {
        int asked = ++request;
        page(activity.getString(R.string.coop_loading, computer.displayName()));
        worker.execute(() -> {
            try {
                NativeCoop coop = client.open().coop();
                main.execute(() -> listed(asked, coop));
            } catch (NativeApiException e) {
                main.execute(() -> listFailed(asked, e));
            }
        });
    }

    private boolean speaks(int asked, String outcome) {
        if (closed || asked != request) {
            LimeLog.info("Co-op " + outcome + " after the sheet closed or asked again; not shown");
            return false;
        }
        return true;
    }

    private void listed(int asked, NativeCoop coop) {
        if (!speaks(asked, "list")) return;
        if (!coop.available) {
            LimeLog.warning("Co-op unavailable on " + computer.uuid + ": " + coop.reason);
            close();
            ui.problem(activity.getString(R.string.coop_problem_unavailable, computer.displayName()),
                    coop.reason != null && !coop.reason.isEmpty() ? coop.reason
                            : activity.getString(R.string.coop_unavailable_no_reason), null);
            return;
        }
        shown = coop;
        render(coop);
    }

    private void listFailed(int asked, NativeApiException error) {
        if (!speaks(asked, "list failure")) return;
        NativeCoop.Problem problem = NativeCoop.Problem.of(error);
        LimeLog.warning("Co-op list failed: " + problem + " " + error.getKind() + " " + error.getMessage());
        close();
        ui.problem(problemTitle(activity, problem, computer.displayName()), problemMessage(activity, error),
                problem == NativeCoop.Problem.UNREACHABLE ? ui::reopen : null);
    }

    private void render(NativeCoop coop) {
        sheet.clearActions();
        if (coop.sessions.isEmpty()) {
            sheet.addNote(sheet.addGroup(), activity.getString(R.string.coop_none), false);
        }
        for (NativeCoop.Session session : coop.sessions) {
            sheet.addSection(session.title);
            sheet.addInfo(activity.getString(R.string.coop_status), phaseText(activity, session.phase));
            for (NativeCoop.Player player : session.players) {
                sheet.addInfo(activity.getString(R.string.coop_player, player.slot + 1), playerText(activity, player));
            }
            if (session.endReason != null) {
                sheet.addNote(sheet.addGroup(), endText(activity, session), true);
            }
            NativeCoop.Player seat = session.seat();
            MaterialButton action;
            if (seat != null) {
                action = sheet.addAction(R.drawable.ic_play, activity.getString(R.string.coop_return),
                        activity.getString(R.string.coop_return_note, seat.slot + 1));
            } else {
                action = sheet.addAction(R.drawable.ic_gamepad, activity.getString(R.string.coop_join),
                        joinNote(activity, session));
            }
            if (!session.joinable) {
                // Dimmed but reachable by D-pad and TalkBack: choosing it asks the host, whose answer says why.
                action.setAlpha(HostMenu.DISABLED_ALPHA);
            }
            action.setOnClickListener(v -> join(session));
        }
        sheet.addAction(R.drawable.ic_refresh, activity.getString(R.string.coop_refresh)).setOnClickListener(v -> load());
        sheet.focusFirstAction();
    }

    private void join(NativeCoop.Session session) {
        // A double tap or a key repeat must not join twice.
        if (joining || closed) return;
        joining = true;
        int asked = ++request;
        page(activity.getString(R.string.coop_joining, session.title));
        worker.execute(() -> {
            try {
                NativeCoop.Joined joined = client.open().joinCoop(session.id);
                main.execute(() -> joined(asked, session, joined));
            } catch (NativeApiException e) {
                main.execute(() -> joinFailed(asked, session, e));
            }
        });
    }

    private void joined(int asked, NativeCoop.Session session, NativeCoop.Joined joined) {
        joining = false;
        LimeLog.info("Co-op: joined session " + joined.session + " in slot " + joined.slot + ", app " + joined.appId);
        if (!speaks(asked, "join")) return;
        String title = shown != null ? shown.entryTitle(joined.appId) : null;
        NvApp app = new NvApp(title != null ? title : session.title, joined.appId, false);
        close();
        ui.start(app);
    }

    private void joinFailed(int asked, NativeCoop.Session session, NativeApiException error) {
        joining = false;
        NativeCoop.Problem problem = NativeCoop.Problem.of(error);
        LimeLog.warning("Co-op join failed: " + problem + " " + error.getKind() + " " + error.getMessage());
        if (!speaks(asked, "join failure")) return;
        CharSequence title = problemTitle(activity, problem, computer.displayName());
        CharSequence message = problemMessage(activity, error);
        switch (problem) {
            case UNREACHABLE:
                render(shown);
                ui.problem(title, message, () -> join(session));
                break;
            case PERMISSION:
                // The list stays as it is: the permission is the device's, not the session's.
                render(shown);
                ui.problem(title, message, null);
                break;
            case REFUSED:
            case GONE:
            case BUSY:
                // The session changed since it was listed: show it as it is now.
                ui.problem(title, message, null);
                load();
                break;
            default:
                close();
                ui.problem(title, message, null);
                break;
        }
    }

    private void page(String note) {
        sheet.clearActions();
        sheet.addNote(sheet.addGroup(), note, false);
    }

    static String problemTitle(Context context, NativeCoop.Problem problem, String pc) {
        switch (problem) {
            case PERMISSION:
                return context.getString(R.string.coop_problem_permission);
            case REFUSED:
                return context.getString(R.string.coop_problem_refused, pc);
            case GONE:
                return context.getString(R.string.coop_problem_gone);
            case BUSY:
                return context.getString(R.string.coop_problem_busy, pc);
            case UNAVAILABLE:
                return context.getString(R.string.coop_problem_unavailable, pc);
            case UNREACHABLE:
                return context.getString(R.string.coop_problem_unreachable, pc);
            default:
                return context.getString(R.string.coop_problem_other);
        }
    }

    /** The host's own words, or the status when it sent none. */
    static String problemMessage(Context context, NativeApiException error) {
        String message = error.getMessage();
        if (message != null && !message.isEmpty()) {
            return message;
        }
        return context.getString(R.string.coop_problem_no_message, error.getHttpStatus());
    }

    static String phaseText(Context context, NativeCoop.Phase phase) {
        switch (phase) {
            case STARTING:
                return context.getString(R.string.coop_phase_starting);
            case RUNNING:
                return context.getString(R.string.coop_phase_running);
            case ENDED:
                return context.getString(R.string.coop_phase_ended);
            default:
                return context.getString(R.string.coop_phase_unknown);
        }
    }

    static String playerText(Context context, NativeCoop.Player player) {
        if (player.isFree()) {
            return context.getString(R.string.coop_player_free);
        }
        String name = player.name != null ? player.name : context.getString(R.string.coop_player_unnamed);
        if (player.you) {
            name = context.getString(R.string.coop_player_you, name);
        }
        return player.away ? context.getString(R.string.coop_player_away, name) : name;
    }

    static String endText(Context context, NativeCoop.Session session) {
        int reason;
        switch (session.endReason) {
            case ENDED:
                reason = R.string.coop_end_ended;
                break;
            case EMPTY:
                reason = R.string.coop_end_empty;
                break;
            case CRASHED:
                reason = R.string.coop_end_crashed;
                break;
            case FAILED:
                reason = R.string.coop_end_failed;
                break;
            default:
                reason = R.string.coop_end_unknown;
                break;
        }
        String text = context.getString(reason);
        return session.endDetail != null && !session.endDetail.isEmpty() ? text + "\n" + session.endDetail : text;
    }

    /** What a join does here: take a free slot, or why it can't, in the host's words. */
    static String joinNote(Context context, NativeCoop.Session session) {
        if (session.joinable) {
            return context.getResources().getQuantityString(R.plurals.coop_free_slots, session.freeSlots,
                    session.freeSlots);
        }
        return session.refusal != null && !session.refusal.isEmpty() ? session.refusal
                : context.getString(R.string.coop_cannot_join);
    }
}
