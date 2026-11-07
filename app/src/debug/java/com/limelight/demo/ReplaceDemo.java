package com.limelight.demo;

import android.content.Context;

/** Package hop to the running-game prompt, for the dialog demo. */
final class ReplaceDemo {
    private ReplaceDemo() { }

    static void show(Context context) {
        new com.limelight.ui.NavigationDialogBuilder(context)
                .setMessage("Racing Game is running. Close it and start Open World?")
                .setPositiveButton("Close and start", null)
                .setNeutralButton("Resume Racing Game", null)
                .setNegativeButton(android.R.string.cancel, null).show();
    }
}
