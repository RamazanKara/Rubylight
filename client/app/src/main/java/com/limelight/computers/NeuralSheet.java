package com.limelight.computers;

import android.app.Activity;
import android.content.Context;
import android.os.Handler;
import android.os.Looper;

import androidx.annotation.VisibleForTesting;

import com.google.android.material.button.MaterialButton;
import com.limelight.LimeLog;
import com.limelight.R;
import com.limelight.binding.PlatformBinding;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.JniNativeApiTransport;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativeNeural;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.ui.ActionSheet;
import com.limelight.ui.NavigationDialogBuilder;

import java.util.concurrent.Executor;

/**
 * A game's Neural Rendering from the library's game menu, offered while the host lists
 * {@code neural.frame_generation}: the frame generation switch, the host's one-time notice before the first install
 * (Cancel is the default), what turning it on replaces, and the host's job while it downloads, copies or removes.
 *
 * <p>Host calls run on {@code worker}; only the answer to the newest call while the sheet is open is shown. While the
 * host works the sheet reads again every {@link #POLL_MS}.
 */
public final class NeuralSheet {
    interface Client {
        NativeApiClient open() throws NativeApiException;
    }

    interface Scheduler {
        void later(Runnable task, long delayMs);
    }

    static final long POLL_MS = 1500;

    private final Activity activity;
    private final String app;
    private final Client client;
    private final Executor worker;
    private final Executor main;
    private final Scheduler scheduler;
    private final ActionSheet sheet;
    // Main thread only.
    private int request;
    private boolean closed;
    private boolean writing;
    private NativeNeural shown;

    /** Opens the sheet for {@code appUuid}, the library's app id. */
    public static NeuralSheet show(Activity activity, ComputerDetails computer, String uniqueId, String appUuid,
                                   String gameName) {
        Context context = activity.getApplicationContext();
        Handler handler = new Handler(Looper.getMainLooper());
        NeuralSheet sheet = new NeuralSheet(activity, appUuid, gameName, () -> {
            HostEndpoint endpoint = HostEndpoint.of(computer, uniqueId, PlatformBinding.getCryptoProvider(context));
            if (endpoint == null) {
                throw new NativeApiException(NativeApiException.Kind.NOT_PAIRED,
                        "No address or pinned certificate for the PC");
            }
            return new NativeApiClient(new JniNativeApiTransport(), endpoint);
        }, task -> new Thread(task, "Neural Rendering").start(), handler::post, handler::postDelayed);
        sheet.show();
        return sheet;
    }

    @VisibleForTesting
    NeuralSheet(Activity activity, String app, String gameName, Client client, Executor worker, Executor main,
                Scheduler scheduler) {
        this.activity = activity;
        this.app = app;
        this.client = client;
        this.worker = worker;
        this.main = main;
        this.scheduler = scheduler;
        sheet = new ActionSheet(activity, activity.getString(R.string.neural_title, gameName));
        sheet.setOnClosed(() -> closed = true);
    }

    @VisibleForTesting
    ActionSheet sheet() {
        return sheet;
    }

    @VisibleForTesting
    NativeNeural shown() {
        return shown;
    }

    void show() {
        sheet.show();
        page(activity.getString(R.string.neural_loading));
        load();
    }

    private void load() {
        int asked = ++request;
        worker.execute(() -> {
            try {
                NativeNeural state = client.open().neural(app);
                main.execute(() -> answered(asked, state));
            } catch (NativeApiException e) {
                main.execute(() -> failed(asked, e, false));
            }
        });
    }

    private void write(boolean on, boolean acceptNotice) {
        if (writing || closed) return;
        writing = true;
        int asked = ++request;
        page(activity.getString(on ? R.string.neural_turning_on : R.string.neural_turning_off));
        worker.execute(() -> {
            try {
                NativeNeural state = client.open().writeNeural(app, on, acceptNotice);
                main.execute(() -> {
                    writing = false;
                    answered(asked, state);
                });
            } catch (NativeApiException e) {
                main.execute(() -> {
                    writing = false;
                    failed(asked, e, true);
                });
            }
        });
    }

    private boolean speaks(int asked) {
        return !closed && asked == request;
    }

    private void answered(int asked, NativeNeural state) {
        if (!speaks(asked)) return;
        shown = state;
        render(state);
        if (state.working()) {
            scheduler.later(() -> {
                if (speaks(asked)) load();
            }, POLL_MS);
        }
    }

    private void failed(int asked, NativeApiException error, boolean wrote) {
        LimeLog.warning("Neural Rendering " + (wrote ? "write" : "read") + " failed: " + error.getKind() + " "
                + error.getCode() + " " + error.getMessage());
        if (!speaks(asked)) return;
        if (shown != null) {
            render(shown);
        } else {
            sheet.dismiss();
        }
        new NavigationDialogBuilder(activity)
                .setTitle(R.string.neural_problem)
                .setMessage(CoopSheet.problemMessage(activity, error))
                .setPositiveButton(android.R.string.ok, null)
                .show();
        if (wrote) load();
    }

    private void render(NativeNeural state) {
        sheet.clearActions();
        sheet.addNote(sheet.addGroup(), activity.getString(R.string.neural_about), false);
        NativeNeural.Switch fg = state.frameGeneration;
        if (fg == null) {
            sheet.addNote(sheet.addGroup(), activity.getString(R.string.neural_not_offered), false);
        } else {
            MaterialButton action = sheet.addAction(R.drawable.ic_tune,
                    activity.getString(R.string.neural_frame_generation), switchNote(activity, fg));
            if (!fg.configurable()) {
                // Dimmed but reachable by D-pad and TalkBack: choosing it says why.
                action.setAlpha(HostMenu.DISABLED_ALPHA);
            }
            action.setOnClickListener(v -> toggle(state));
            if (state.replacesEarlierUpscaling && !fg.on) {
                sheet.addNote(sheet.addGroup(), activity.getString(R.string.neural_replaces), false);
            }
        }
        String job = jobNote(activity, state);
        if (job != null) {
            sheet.addNote(sheet.addGroup(), job, state.failed());
        }
        sheet.addAction(R.drawable.ic_refresh, activity.getString(R.string.neural_refresh)).setOnClickListener(v -> {
            page(activity.getString(R.string.neural_loading));
            load();
        });
        sheet.focusFirstAction();
    }

    private void toggle(NativeNeural state) {
        NativeNeural.Switch fg = state.frameGeneration;
        if (fg == null || writing) return;
        if (!fg.configurable()) {
            new NavigationDialogBuilder(activity)
                    .setTitle(R.string.neural_frame_generation)
                    .setMessage(fg.reason != null ? fg.reason : activity.getString(R.string.neural_unavailable))
                    .setPositiveButton(android.R.string.ok, null)
                    .show();
            return;
        }
        if (fg.on) {
            write(false, false);
        } else if (state.accepted) {
            write(true, false);
        } else {
            // The host's one-time notice; Cancel is the default.
            androidx.appcompat.app.AlertDialog notice = new NavigationDialogBuilder(activity)
                    .setTitle(R.string.neural_notice_title)
                    .setMessage(activity.getString(R.string.neural_notice))
                    .setNegativeButton(android.R.string.cancel, null)
                    .setPositiveButton(R.string.neural_notice_accept, (d, w) -> write(true, true))
                    .show();
            android.widget.Button cancel = notice.getButton(android.content.DialogInterface.BUTTON_NEGATIVE);
            if (cancel != null) cancel.requestFocus();
        }
    }

    private void page(String note) {
        sheet.clearActions();
        sheet.addNote(sheet.addGroup(), note, false);
    }

    static String switchNote(Context context, NativeNeural.Switch fg) {
        if (!fg.configurable() && fg.reason != null && !fg.reason.isEmpty()) return fg.reason;
        return context.getString(fg.on ? R.string.neural_on : R.string.neural_off);
    }

    /** What the host is doing for the game, or why it last failed; null when there is nothing to say. */
    static String jobNote(Context context, NativeNeural state) {
        if (state.phase == null) return null;
        switch (state.phase) {
            case "download":
                return state.total > 0 ? context.getString(R.string.neural_downloading_of,
                        state.done / 1e6f, state.total / 1e6f) : context.getString(R.string.neural_downloading);
            case "place":
                return context.getString(R.string.neural_copying);
            case "remove":
                return context.getString(R.string.neural_removing);
            case "failed":
                return state.error != null && !state.error.isEmpty() ? state.error
                        : context.getString(R.string.neural_failed);
            default:
                return null;
        }
    }
}
