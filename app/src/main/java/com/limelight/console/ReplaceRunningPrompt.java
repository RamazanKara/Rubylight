package com.limelight.console;

import android.content.Context;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.limelight.R;

/**
 * What tapping a game asks while another game runs on the same PC: resume the running one, close it and
 * start this one, or cancel. Starting replaces the running game, so unsaved progress there is lost.
 */
final class ReplaceRunningPrompt {
    private ReplaceRunningPrompt() {
    }

    static AlertDialog build(Context context, String running, String starting, Runnable resume, Runnable replace) {
        return new MaterialAlertDialogBuilder(context)
                .setMessage(context.getString(R.string.library_replace_message_named, running, starting))
                .setPositiveButton(R.string.library_replace_confirm, (dialog, which) -> replace.run())
                .setNeutralButton(context.getString(R.string.library_replace_resume, running),
                        (dialog, which) -> resume.run())
                .setNegativeButton(android.R.string.cancel, null).create();
    }
}
