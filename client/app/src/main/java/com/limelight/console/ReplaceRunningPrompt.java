package com.limelight.console;

import android.content.Context;
import android.content.DialogInterface;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;

/**
 * What tapping a game asks while another game runs on the same PC: "Resume X", "Close X and start Y" or
 * Cancel. Starting replaces the running game, so unsaved progress there is lost; focus therefore starts on
 * Resume (or on Cancel when nothing can be resumed), never on the button that destroys it.
 */
final class ReplaceRunningPrompt {
    private ReplaceRunningPrompt() {
    }

    /**
     * {@code resume} is null when the running game cannot be resumed from here; the button is left out.
     * {@code running} is null when the PC does not say which game runs; the prompt then does without its name.
     */
    static AlertDialog build(Context context, String running, String starting, Runnable resume, Runnable replace) {
        androidx.appcompat.app.AlertDialog.Builder builder = new com.limelight.ui.NavigationDialogBuilder(context)
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
