package com.limelight.console;

import android.app.Activity;
import android.content.Context;
import android.content.DialogInterface;
import android.widget.Toast;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import java.util.function.Consumer;

/**
 * What tapping a game asks while another game runs on the same PC: "Resume X", "Close X and start Y" or
 * Cancel. Starting replaces the running game, so unsaved progress there is lost; focus therefore starts on
 * Resume (or on Cancel when nothing can be resumed), never on the button that destroys it.
 */
final class ReplaceRunningPrompt {
    private ReplaceRunningPrompt() {
    }

    /** What the screen asking does with the answer. */
    interface Screen {
        /** The PC as it is known now, or null when it is gone. */
        ComputerDetails currentHost();

        void start(NvApp app, ComputerDetails computer);

        void resume(NvApp running, ComputerDetails computer);

        /** Closes the game on the PC; {@code onDone} learns on any thread whether it is closed. */
        void close(NvApp running, ComputerDetails computer, Consumer<Boolean> onDone);

        /** Another game than the one named runs now: the screen reads the PC again. */
        void reload();
    }

    /** The prompt for {@code plan}, acting on what the PC runs when it is answered. {@code activity} is the screen's own. */
    static AlertDialog ask(Activity activity, PlayPlan plan, String starting, NvApp app, Screen screen) {
        NvApp running = plan.runningApp();
        return build(activity, plan.runningTitle, starting,
                plan.canResume ? () -> resume(activity, running, screen) : null,
                () -> replace(activity, plan, running, app, screen));
    }

    /** What the host runs may have changed while the prompt was open: Resume only acts on what was named. */
    private static void resume(Activity activity, NvApp running, Screen screen) {
        ComputerDetails computer = screen.currentHost();
        if (computer == null || PlayPlan.recheck(running, computer.runningGameId, computer.runningGameUuid)
                != PlayPlan.Check.SAME) {
            changed(activity, screen);
            return;
        }
        screen.resume(running, computer);
    }

    private static void replace(Activity activity, PlayPlan plan, NvApp running, NvApp app, Screen screen) {
        ComputerDetails computer = screen.currentHost();
        if (computer == null) {
            return;
        }
        PlayPlan.Check check = PlayPlan.recheck(running, computer.runningGameId, computer.runningGameUuid);
        if (check == PlayPlan.Check.CHANGED) {
            // Another game than the one named runs now: closing it would not be what the player agreed to.
            changed(activity, screen);
            return;
        }
        if (check == PlayPlan.Check.SAME && plan.closeNatively) {
            // Like Close game: through the native API, and only a closed game is followed by the start.
            screen.close(running, computer, closed -> {
                if (closed) {
                    activity.runOnUiThread(() -> {
                        if (!activity.isFinishing() && !activity.isDestroyed()) {
                            screen.start(app, computer);
                        }
                    });
                }
            });
            return;
        }
        // Gone already, or the classic quit: the connection quits what runs, then starts.
        screen.start(app, computer);
    }

    private static void changed(Activity activity, Screen screen) {
        Toast.makeText(activity, R.string.library_replace_changed, Toast.LENGTH_LONG).show();
        screen.reload();
    }

    /**
     * {@code resume} is null when the running game cannot be resumed from here; the button is left out.
     * {@code running} is null when the PC does not say which game runs; the prompt then does without its name.
     */
    static AlertDialog build(Context context, String running, String starting, Runnable resume, Runnable replace) {
        androidx.appcompat.app.AlertDialog.Builder builder = new com.limelight.ui.NavigationDialogBuilder(context).startOnCancel()
                .setMessage(running == null ? context.getString(R.string.library_replace_message, starting)
                        : context.getString(R.string.library_replace_message_running, running, starting))
                .setPositiveButton(running == null ? context.getString(R.string.library_replace_confirm)
                                : context.getString(R.string.library_replace_close_start, running, starting),
                        (dialog, which) -> replace.run())
                .setNegativeButton(android.R.string.cancel, null);
        if (resume != null) {
            builder.setNeutralButton(context.getString(R.string.library_replace_resume, running),
                    (dialog, which) -> resume.run());
        }
        AlertDialog dialog = builder.create();
        dialog.setOnShowListener(shown -> dialog.getButton(
                resume != null ? DialogInterface.BUTTON_NEUTRAL : DialogInterface.BUTTON_NEGATIVE).requestFocus());
        return dialog;
    }
}
